use serde_json::Value;

/// Something that happened in a [`crate::Events`] stream's browser.
#[derive(Clone, Debug)]
pub enum BrowserEvent {
    /// After a load commits/finishes, and on SPA navigations
    /// (pushState / replaceState / popstate / hashchange).
    Navigated { url: String, title: String },
    Loading(bool),
    /// Inspect mode: the user clicked an element.
    Picked(Picked),
    /// Inspect mode: the user selected page text.
    Text { text: String, url: String },
    /// The user pressed Esc in the page while inspecting; inspect is now off.
    InspectEnded,
    /// A load failed (e.g. connection refused), with a readable message.
    Error(String),
    /// The user clicked in the page (it has the keyboard now).
    Focused,
}

/// Edit commands sent to the page (Cmd+C and friends while it has focus).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditCommand {
    Copy,
    Cut,
    Paste,
    SelectAll,
    Undo,
    Redo,
}

/// An element the user picked in inspect mode.
#[derive(Clone, Debug, Default)]
pub struct Picked {
    pub selector: String,
    pub tag: String,
    pub id: Option<String>,
    pub classes: Vec<String>,
    /// innerText, trimmed to 300 chars.
    pub text: String,
    /// outerHTML, truncated to 3000 chars with an ellipsis marker.
    pub outer_html: String,
    /// Key computed styles, in a fixed order.
    pub styles: Vec<(String, String)>,
    /// x, y, w, h in CSS px relative to the viewport.
    pub rect: [f64; 4],
    pub url: String,
    /// `data-qur-source` / `data-source`, or React dev `_debugSource` as `file:line`.
    pub source: Option<String>,
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn opt(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
}

/// Parse one JSON message posted by the injected script.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn parse_message(json: &str) -> Option<BrowserEvent> {
    let v: Value = serde_json::from_str(json).ok()?;
    Some(match v.get("t")?.as_str()? {
        "nav" => BrowserEvent::Navigated { url: s(&v, "url"), title: s(&v, "title") },
        "text" => BrowserEvent::Text { text: s(&v, "text"), url: s(&v, "url") },
        "esc" => BrowserEvent::InspectEnded,
        "focus" => BrowserEvent::Focused,
        "pick" => {
            let mut rect = [0.0; 4];
            if let Some(a) = v.get("rect").and_then(Value::as_array) {
                for (r, x) in rect.iter_mut().zip(a) {
                    *r = x.as_f64().unwrap_or(0.0);
                }
            }
            let strs = |k: &str| -> Vec<Value> {
                v.get(k).and_then(Value::as_array).cloned().unwrap_or_default()
            };
            BrowserEvent::Picked(Picked {
                selector: s(&v, "selector"),
                tag: s(&v, "tag"),
                id: opt(&v, "id"),
                classes: strs("classes")
                    .iter()
                    .filter_map(|c| c.as_str().map(str::to_string))
                    .collect(),
                text: s(&v, "text"),
                outer_html: s(&v, "html"),
                styles: strs("styles")
                    .iter()
                    .filter_map(|p| {
                        let p = p.as_array()?;
                        Some((p.first()?.as_str()?.to_string(), p.get(1)?.as_str()?.to_string()))
                    })
                    .collect(),
                rect,
                url: s(&v, "url"),
                source: opt(&v, "source"),
            })
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pick() {
        let j = r##"{"t":"pick","selector":"#main > div:nth-of-type(2)","tag":"div","id":null,
            "classes":["card","big"],"text":"Hi","html":"<div class=\"card big\">Hi</div>",
            "styles":[["display","flex"],["color","rgb(0, 0, 0)"]],"rect":[1,2.5,30,40],
            "url":"file:///p.html","source":"src/App.tsx:12"}"##;
        let Some(BrowserEvent::Picked(p)) = parse_message(j) else { panic!() };
        assert_eq!(p.selector, "#main > div:nth-of-type(2)");
        assert_eq!(p.tag, "div");
        assert_eq!(p.id, None);
        assert_eq!(p.classes, ["card", "big"]);
        assert_eq!(p.outer_html, r#"<div class="card big">Hi</div>"#);
        assert_eq!(p.styles[0], ("display".into(), "flex".into()));
        assert_eq!(p.rect, [1.0, 2.5, 30.0, 40.0]);
        assert_eq!(p.source.as_deref(), Some("src/App.tsx:12"));
    }

    #[test]
    fn parses_others() {
        assert!(matches!(
            parse_message(r#"{"t":"nav","url":"u","title":"T"}"#),
            Some(BrowserEvent::Navigated { url, title }) if url == "u" && title == "T"
        ));
        assert!(matches!(
            parse_message(r#"{"t":"text","text":"abc","url":"u"}"#),
            Some(BrowserEvent::Text { text, .. }) if text == "abc"
        ));
        assert!(matches!(parse_message(r#"{"t":"esc"}"#), Some(BrowserEvent::InspectEnded)));
        assert!(parse_message(r#"{"t":"bogus"}"#).is_none());
        assert!(parse_message("not json").is_none());
        assert!(parse_message(r#"{"x":1}"#).is_none());
    }

    #[test]
    fn pick_tolerates_missing_fields() {
        let Some(BrowserEvent::Picked(p)) = parse_message(r#"{"t":"pick","id":"x"}"#) else {
            panic!()
        };
        assert_eq!(p.id.as_deref(), Some("x"));
        assert!(p.classes.is_empty() && p.styles.is_empty() && p.source.is_none());
    }
}
