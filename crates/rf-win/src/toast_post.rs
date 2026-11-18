//! Our own toast notifications (Ferry's questions and arrivals). `toasts` reads other programs' notifications; this module posts ours.
//!
//! Windows shows a toast only for an AppUserModelID that a Start menu shortcut carries: the installed app has one (Velopack's
//! shortcut, `velopack.rustyfiles`), a build run from the source tree has none, and then [`show`] fails and the caller asks in a
//! window of its own instead. It fails too when the user turned notifications off for us or altogether.
//!
//! The buttons' answers come back through the toast's Activated event while this process runs. There is no COM activator, so a
//! click after the process ended does nothing (Ferry's questions live only while it runs anyway).
//!
//! Every text that goes into the toast is escaped: device and file names come from other machines and must never be read as XML.

use std::collections::HashMap;
use std::sync::Mutex;

use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::{NotificationSetting, ToastActivatedEventArgs, ToastNotification, ToastNotificationManager};
use windows::core::{HSTRING, IInspectable, Interface};

const GROUP: &str = "rustyfiles";

/// The toasts shown and not yet taken away, by tag: kept so their Activated handlers stay alive.
static LIVE: Mutex<Option<HashMap<String, ToastNotification>>> = Mutex::new(None);

pub struct Button {
    pub label: String,
    /// What `on_activate` gets when it is pressed.
    pub argument: String,
}

/// What a toast says and offers.
pub struct Toast<'a> {
    /// Names it: a later toast with the same tag replaces it, [`remove`] takes it away.
    pub tag: &'a str,
    pub title: &'a str,
    pub lines: &'a [&'a str],
    /// What `on_activate` gets for a click on the toast itself.
    pub launch: &'a str,
    pub buttons: &'a [Button],
    /// Stays on screen until it is answered (a question), instead of going to the notification list after a few seconds.
    pub stays: bool,
}

/// Shows a toast. `on_activate` (called on a thread of Windows') gets the argument of the button pressed, or `launch`.
pub fn show(app_id: &str, toast: &Toast, on_activate: impl Fn(String) + Send + Sync + 'static) -> windows::core::Result<()> {
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(app_id))?;
    if notifier.Setting()? != NotificationSetting::Enabled {
        return Err(windows::core::Error::from_hresult(windows::Win32::Foundation::E_ACCESSDENIED));
    }
    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(xml(toast)))?;
    let n = ToastNotification::CreateToastNotification(&doc)?;
    n.SetTag(&HSTRING::from(toast.tag))?;
    n.SetGroup(&HSTRING::from(GROUP))?;
    n.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(move |_, args| {
        if let Some(args) = args.as_ref()
            && let Ok(args) = args.cast::<ToastActivatedEventArgs>()
            && let Ok(a) = args.Arguments()
        {
            on_activate(a.to_string());
        }
        Ok(())
    }))?;
    notifier.Show(&n)?;
    LIVE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(toast.tag.to_string(), n);
    Ok(())
}

/// Takes a toast away (answered elsewhere, or no longer asked).
pub fn remove(app_id: &str, tag: &str) {
    if let Some(live) = LIVE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        live.remove(tag);
    }
    if let Ok(history) = ToastNotificationManager::History() {
        let _ = history.RemoveGroupedTagWithId(&HSTRING::from(tag), &HSTRING::from(GROUP), &HSTRING::from(app_id));
    }
}

/// The toast's XML (`ToastGeneric`). Pure, for the tests.
pub fn xml(t: &Toast) -> String {
    let mut s = String::new();
    s.push_str(&format!(r#"<toast launch="{}""#, escape(t.launch)));
    if t.stays {
        s.push_str(r#" scenario="reminder""#);
    }
    s.push_str(r#"><visual><binding template="ToastGeneric">"#);
    s.push_str(&format!("<text>{}</text>", escape(t.title)));
    for line in t.lines.iter().take(2) {
        s.push_str(&format!("<text>{}</text>", escape(line)));
    }
    s.push_str("</binding></visual>");
    if !t.buttons.is_empty() {
        s.push_str("<actions>");
        for b in t.buttons.iter().take(5) {
            s.push_str(&format!(r#"<action content="{}" arguments="{}" activationType="foreground"/>"#, escape(&b.label), escape(&b.argument)));
        }
        s.push_str("</actions>");
    }
    s.push_str("</toast>");
    s
}

/// XML text and attribute escaping. Control characters (other than tab) are dropped: XML 1.0 has no place for them.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_other_machines_cannot_break_out_of_the_xml() {
        let evil = r#"Mac"/><action content="x" arguments="ferry:yes:1"/><text>"#;
        let t = Toast {
            tag: "q",
            title: evil,
            lines: &["a & b <c>", "it's"],
            launch: "ferry:open:q",
            buttons: &[Button { label: "Kabul et".into(), argument: "ferry:yes:q".into() }],
            stays: true,
        };
        let x = xml(&t);
        assert_eq!(x.matches("<action ").count(), 1, "{x}");
        assert!(x.contains("Mac&quot;/&gt;&lt;action"), "{x}");
        assert!(x.contains("a &amp; b &lt;c&gt;") && x.contains("it&apos;s"), "{x}");
        assert!(x.contains(r#"scenario="reminder""#));
        // and Windows' own parser takes it
        let doc = XmlDocument::new().unwrap();
        doc.LoadXml(&HSTRING::from(x)).unwrap();
    }

    #[test]
    fn control_characters_are_dropped() {
        assert_eq!(escape("a\u{0}b\u{1b}c\td"), "abc\td");
    }
}
