//! Editing text in other applications through AT-SPI, the Linux accessibility bus.
//!
//! Diktatoren follows focus events the way a screen reader does, so it knows which text field
//! has focus without walking every application's tree. A field that offers AT-SPI's Text and
//! EditableText interfaces can be read (text, cursor, selection) and changed in place.
//!
//! All AT-SPI offsets count characters, not bytes.

use std::sync::OnceLock;
use std::sync::mpsc as std_mpsc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use futures::StreamExt;
use tokio::sync::mpsc;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::{Connection, MatchRule, MessageStream, connection::Builder};

const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const TEXT: &str = "org.a11y.atspi.Text";
const EDITABLE: &str = "org.a11y.atspi.EditableText";
/// AT-SPI state bits.
const STATE_FOCUSED: u32 = 12;

/// A text field in another application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    bus: String,
    path: OwnedObjectPath,
}

/// The field's content and where the cursor is, in characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub text: String,
    pub caret: usize,
    pub selection: Option<(usize, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    Delete { start: usize, end: usize },
    Insert { at: usize, text: String },
    Caret(usize),
    Select { start: usize, end: usize },
}

enum Request {
    Focused(std_mpsc::Sender<Option<Target>>),
    Read(Target, std_mpsc::Sender<Result<Snapshot>>),
    Write(Target, Vec<Edit>, std_mpsc::Sender<Result<()>>),
}

fn sender() -> &'static mpsc::UnboundedSender<Request> {
    static TX: OnceLock<mpsc::UnboundedSender<Request>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::unbounded_channel();
        thread::Builder::new()
            .name("a11y".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build();
                match runtime {
                    Ok(rt) => rt.block_on(run(rx)),
                    Err(e) => log::error!("accessibility runtime: {e}"),
                }
            })
            .expect("spawning accessibility thread");
        tx
    })
}

fn ask<T>(make: impl FnOnce(std_mpsc::Sender<T>) -> Request) -> Option<T> {
    let (tx, rx) = std_mpsc::channel();
    sender().send(make(tx)).ok()?;
    rx.recv_timeout(Duration::from_millis(800)).ok()
}

/// Switches the desktop's accessibility on, so applications started from now on expose their
/// text, and starts following focus.
pub fn start() {
    let _ = sender();
}

/// The focused text field, when it can be edited in place.
pub fn focused() -> Option<Target> {
    ask(Request::Focused).flatten()
}

pub fn read(target: &Target) -> Result<Snapshot> {
    ask(|tx| Request::Read(target.clone(), tx)).ok_or_else(|| anyhow!("the application did not answer"))?
}

pub fn write(target: &Target, edits: Vec<Edit>) -> Result<()> {
    ask(|tx| Request::Write(target.clone(), edits, tx)).ok_or_else(|| anyhow!("the application did not answer"))?
}

async fn run(mut rx: mpsc::UnboundedReceiver<Request>) {
    let bus = match connect().await {
        Ok(bus) => bus,
        Err(e) => {
            log::warn!("accessibility unavailable: {e:#}");
            // Answer every request with nothing, so callers fall back to the dictation window.
            while let Some(request) = rx.recv().await {
                match request {
                    Request::Focused(tx) => drop(tx.send(None)),
                    Request::Read(_, tx) => drop(tx.send(Err(anyhow!("accessibility unavailable")))),
                    Request::Write(_, _, tx) => drop(tx.send(Err(anyhow!("accessibility unavailable")))),
                }
            }
            return;
        }
    };
    let focus = std::sync::Arc::new(std::sync::Mutex::new(None::<Target>));
    tokio::spawn(follow_focus(bus.clone(), focus.clone()));
    while let Some(request) = rx.recv().await {
        match request {
            Request::Focused(tx) => {
                let candidate = focus.lock().unwrap().clone();
                let found = match candidate {
                    Some(t) if editable_and_focused(&bus, &t).await => Some(t),
                    // No focus event seen yet, for example just after starting: look for it.
                    _ => find_focused(&bus).await,
                };
                if let Some(t) = &found {
                    *focus.lock().unwrap() = Some(t.clone());
                }
                let _ = tx.send(found);
            }
            Request::Read(target, tx) => {
                let _ = tx.send(snapshot(&bus, &target).await);
            }
            Request::Write(target, edits, tx) => {
                let _ = tx.send(apply(&bus, &target, edits).await);
            }
        }
    }
}

/// Turns accessibility on and connects to the accessibility bus.
async fn connect() -> Result<Connection> {
    let session = Connection::session().await?;
    let status = zbus::Proxy::new(&session, "org.a11y.Bus", "/org/a11y/bus", "org.a11y.Status").await?;
    if let Err(e) = status.set_property("IsEnabled", true).await {
        log::warn!("could not switch accessibility on: {e}");
    }
    let launcher = zbus::Proxy::new(&session, "org.a11y.Bus", "/org/a11y/bus", "org.a11y.Bus").await?;
    let address: String = launcher.call("GetAddress", &()).await.context("finding the accessibility bus")?;
    Ok(Builder::address(address.as_str())?.build().await?)
}

/// Remembers the last object that reported gaining focus.
async fn follow_focus(bus: Connection, focus: std::sync::Arc<std::sync::Mutex<Option<Target>>>) {
    // Applications only send events someone has registered for.
    let registry = zbus::Proxy::new(&bus, "org.a11y.atspi.Registry", "/org/a11y/atspi/registry", "org.a11y.atspi.Registry").await;
    if let Ok(registry) = registry {
        let event = "object:state-changed:focused";
        let registered: zbus::Result<()> = registry.call("RegisterEvent", &(event, Vec::<String>::new(), "")).await;
        if registered.is_err() {
            let _: zbus::Result<()> = registry.call("RegisterEvent", &(event,)).await;
        }
    }
    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.a11y.atspi.Event.Object")
        .and_then(|b| b.member("StateChanged"))
        .map(|b| b.build());
    let Ok(rule) = rule else { return };
    let Ok(mut stream) = MessageStream::for_match_rule(rule, &bus, None).await else {
        log::warn!("cannot follow accessibility focus events");
        return;
    };
    while let Some(Ok(message)) = stream.next().await {
        let header = message.header();
        let (Some(sender), Some(path)) = (header.sender(), header.path()) else { continue };
        let Ok((kind, gained, ..)) = message.body().deserialize::<(String, i32, i32, OwnedValue)>() else { continue };
        if kind == "focused" && gained == 1 {
            *focus.lock().unwrap() = Some(Target { bus: sender.to_string(), path: path.to_owned().into() });
        }
    }
}

async fn proxy<'a>(bus: &'a Connection, t: &'a Target, interface: &'a str) -> zbus::Result<zbus::Proxy<'a>> {
    zbus::Proxy::new(bus, t.bus.as_str(), t.path.as_str(), interface).await
}

async fn editable_and_focused(bus: &Connection, t: &Target) -> bool {
    let Ok(accessible) = proxy(bus, t, ACCESSIBLE).await else { return false };
    let interfaces: Vec<String> = accessible.call("GetInterfaces", &()).await.unwrap_or_default();
    let state: Vec<u32> = accessible.call("GetState", &()).await.unwrap_or_default();
    let focused = state.get((STATE_FOCUSED / 32) as usize).is_some_and(|w| w & (1 << (STATE_FOCUSED % 32)) != 0);
    focused && interfaces.iter().any(|i| i == TEXT) && interfaces.iter().any(|i| i == EDITABLE)
}

/// Walks each application's tree (within limits) for a focused editable text field.
async fn find_focused(bus: &Connection) -> Option<Target> {
    let root = Target { bus: "org.a11y.atspi.Registry".into(), path: OwnedObjectPath::try_from("/org/a11y/atspi/accessible/root").ok()? };
    let apps: Vec<(String, OwnedObjectPath)> = proxy(bus, &root, ACCESSIBLE).await.ok()?.call("GetChildren", &()).await.ok()?;
    for (app, path) in apps {
        let mut budget = 3000;
        if let Some(t) = search(bus, Target { bus: app, path }, 0, &mut budget).await {
            return Some(t);
        }
    }
    None
}

fn search<'a>(
    bus: &'a Connection,
    node: Target,
    depth: usize,
    budget: &'a mut usize,
) -> std::pin::Pin<Box<dyn Future<Output = Option<Target>> + 'a>> {
    Box::pin(async move {
        if depth > 40 || *budget == 0 {
            return None;
        }
        *budget -= 1;
        let accessible = proxy(bus, &node, ACCESSIBLE).await.ok()?;
        let state: Vec<u32> = accessible.call("GetState", &()).await.unwrap_or_default();
        let has = |bit: u32| state.get((bit / 32) as usize).is_some_and(|w| w & (1 << (bit % 32)) != 0);
        // Only descend into what is on screen.
        const STATE_SHOWING: u32 = 25;
        if depth > 1 && !has(STATE_SHOWING) && !has(STATE_FOCUSED) {
            return None;
        }
        if has(STATE_FOCUSED) && editable_and_focused(bus, &node).await {
            return Some(node);
        }
        let children: Vec<(String, OwnedObjectPath)> = accessible.call("GetChildren", &()).await.unwrap_or_default();
        for (child_bus, child_path) in children {
            if let Some(found) = search(bus, Target { bus: child_bus, path: child_path }, depth + 1, budget).await {
                return Some(found);
            }
        }
        None
    })
}

async fn snapshot(bus: &Connection, t: &Target) -> Result<Snapshot> {
    let text_proxy = proxy(bus, t, TEXT).await?;
    let text: String = text_proxy.call("GetText", &(0i32, -1i32)).await?;
    let caret: i32 = text_proxy.get_property("CaretOffset").await.unwrap_or(0);
    let selections: i32 = text_proxy.call("GetNSelections", &()).await.unwrap_or(0);
    let selection = if selections > 0 {
        let (start, end): (i32, i32) = text_proxy.call("GetSelection", &(0i32,)).await?;
        (end > start).then_some((start.max(0) as usize, end.max(0) as usize))
    } else {
        None
    };
    Ok(Snapshot { text, caret: caret.max(0) as usize, selection })
}

async fn apply(bus: &Connection, t: &Target, edits: Vec<Edit>) -> Result<()> {
    let editable = proxy(bus, t, EDITABLE).await?;
    let text_proxy = proxy(bus, t, TEXT).await?;
    for edit in edits {
        match edit {
            Edit::Delete { start, end } => {
                let ok: bool = editable.call("DeleteText", &(start as i32, end as i32)).await?;
                if !ok {
                    return Err(anyhow!("the application refused to delete text"));
                }
            }
            Edit::Insert { at, text } => {
                let length = text.chars().count() as i32;
                let ok: bool = editable.call("InsertText", &(at as i32, text.as_str(), length)).await?;
                if !ok {
                    return Err(anyhow!("the application refused to insert text"));
                }
            }
            Edit::Caret(at) => {
                let _: bool = text_proxy.call("SetCaretOffset", &(at as i32,)).await.unwrap_or(false);
            }
            Edit::Select { start, end } => {
                let selections: i32 = text_proxy.call("GetNSelections", &()).await.unwrap_or(0);
                let _: bool = if selections > 0 {
                    text_proxy.call("SetSelection", &(0i32, start as i32, end as i32)).await.unwrap_or(false)
                } else {
                    text_proxy.call("AddSelection", &(start as i32, end as i32)).await.unwrap_or(false)
                };
            }
        }
    }
    Ok(())
}

/// The smallest edit that turns `old` into `new`, plus where the cursor or selection ends up.
/// Offsets are characters, as AT-SPI wants them.
pub fn diff(old: &str, new: &str, selection: std::ops::Range<usize>) -> Vec<Edit> {
    let old_chars: Vec<char> = old.chars().collect();
    let new_chars: Vec<char> = new.chars().collect();
    let prefix = old_chars.iter().zip(&new_chars).take_while(|(a, b)| a == b).count();
    let max_suffix = old_chars.len().min(new_chars.len()) - prefix;
    let suffix = old_chars
        .iter()
        .rev()
        .zip(new_chars.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    let mut edits = Vec::new();
    let old_end = old_chars.len() - suffix;
    let new_end = new_chars.len() - suffix;
    if old_end > prefix {
        edits.push(Edit::Delete { start: prefix, end: old_end });
    }
    if new_end > prefix {
        edits.push(Edit::Insert { at: prefix, text: new_chars[prefix..new_end].iter().collect() });
    }
    let to_chars = |byte: usize| new[..byte.min(new.len())].chars().count();
    let (start, end) = (to_chars(selection.start), to_chars(selection.end));
    edits.push(if start == end { Edit::Caret(start) } else { Edit::Select { start, end } });
    edits
}

/// Converts a character offset in `text` to a byte offset.
pub fn byte_offset(text: &str, chars: usize) -> usize {
    text.char_indices().nth(chars).map_or(text.len(), |(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_edits() {
        assert_eq!(
            diff("hej", "hej verden", 10..10),
            vec![Edit::Insert { at: 3, text: " verden".into() }, Edit::Caret(10)]
        );
        assert_eq!(
            diff("vi ses mandag", "vi ses tirsdag", 14..14),
            vec![Edit::Delete { start: 7, end: 10 }, Edit::Insert { at: 7, text: "tirs".into() }, Edit::Caret(14)]
        );
        assert_eq!(diff("æble", "æble", 0..2), vec![Edit::Select { start: 0, end: 1 }]);
        assert_eq!(byte_offset("æble", 1), 2);
    }
}
