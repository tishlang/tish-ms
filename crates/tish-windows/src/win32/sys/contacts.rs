//! `windows.contacts`: the Windows address book (People), in tish-macos's `macos.contacts` shape.
//! The Contacts API needs package identity: an MSIX-packaged app that declares the `contacts`
//! capability. Unpackaged, `status()` is "restricted" and queries come back empty with an error.
//!
//! - `status()` -> "notDetermined", "restricted", "denied" or "authorized"
//! - `request(cb)`: ask once (Windows shows its prompt); `cb(granted)`
//! - `query(text, limit, cb)`: `cb({ contacts, error })`, contacts whose name matches `text` (name
//!   prefix first, then word prefix), all of them when it's empty. Each is `{ id, name, given,
//!   family, nickname, org, title, emails, phones }`, the last two `[{ label, value }]`.

use tishlang_core::Value;
use windows::ApplicationModel::Contacts::{Contact, ContactEmailKind, ContactManager, ContactPhoneKind, ContactStoreAccessType};
use windows::Security::Authorization::AppCapabilityAccess::{AppCapability, AppCapabilityAccessStatus};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

use super::{arr, in_background, obj, s, str_arg};

fn capability() -> windows::core::Result<AppCapability> {
    AppCapability::Create(&windows::core::HSTRING::from("contacts"))
}

fn status_name() -> &'static str {
    match capability().and_then(|c| c.CheckAccess()) {
        Ok(AppCapabilityAccessStatus::Allowed) => "authorized",
        Ok(AppCapabilityAccessStatus::UserPromptRequired) => "notDetermined",
        Ok(AppCapabilityAccessStatus::DeniedByUser) => "denied",
        _ => "restricted",
    }
}

pub(super) fn status(_a: &[Value]) -> Value {
    s(status_name())
}

/// WinRT calls below block: they run on a worker thread in the multithreaded apartment.
fn on_worker() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

pub(super) fn request(args: &[Value]) -> Value {
    in_background(
        args.first(),
        || {
            on_worker();
            capability().and_then(|c| c.RequestAccessAsync()?.join()).is_ok_and(|st| st == AppCapabilityAccessStatus::Allowed)
        },
        Value::Bool,
    );
    Value::Null
}

struct Person {
    id: String,
    name: String,
    given: String,
    family: String,
    nickname: String,
    org: String,
    title: String,
    emails: Vec<(String, String)>,
    phones: Vec<(String, String)>,
}

fn email_label(k: ContactEmailKind) -> &'static str {
    match k {
        ContactEmailKind::Personal => "home",
        ContactEmailKind::Work => "work",
        _ => "other",
    }
}

fn phone_label(k: ContactPhoneKind) -> &'static str {
    match k {
        ContactPhoneKind::Home => "home",
        ContactPhoneKind::Mobile => "mobile",
        ContactPhoneKind::Work => "work",
        _ => "other",
    }
}

fn person(c: &Contact) -> windows::core::Result<Person> {
    let job = c.JobInfo()?;
    let (org, title) = match job.GetAt(0) {
        Ok(j) => (j.CompanyName()?.to_string(), j.Title()?.to_string()),
        Err(_) => (String::new(), String::new()),
    };
    let mut emails = Vec::new();
    for e in c.Emails()? {
        emails.push((email_label(e.Kind()?).to_string(), e.Address()?.to_string()));
    }
    let mut phones = Vec::new();
    for p in c.Phones()? {
        phones.push((phone_label(p.Kind()?).to_string(), p.Number()?.to_string()));
    }
    Ok(Person {
        id: c.Id()?.to_string(),
        name: c.DisplayName()?.to_string(),
        given: c.FirstName()?.to_string(),
        family: c.LastName()?.to_string(),
        nickname: c.Nickname()?.to_string(),
        org,
        title,
        emails,
        phones,
    })
}

/// 0: the name starts with `q`; 1: a word in it does; 2: anything else the store matched.
fn rank(p: &Person, q: &str) -> u8 {
    let n = p.name.to_lowercase();
    if q.is_empty() || n.starts_with(q) {
        0
    } else if n.split_whitespace().any(|w| w.starts_with(q)) {
        1
    } else {
        2
    }
}

fn find(text: &str, limit: usize) -> Result<Vec<Person>, String> {
    on_worker();
    let e = |e: windows::core::Error| e.message();
    let store = ContactManager::RequestStoreAsyncWithAccessType(ContactStoreAccessType::AllContactsReadOnly).and_then(|op| op.join()).map_err(e)?;
    let found = if text.is_empty() { store.FindContactsAsync() } else { store.FindContactsWithSearchTextAsync(&windows::core::HSTRING::from(text)) };
    let list = found.and_then(|op| op.join()).map_err(e)?;
    let mut out: Vec<Person> = list.into_iter().filter_map(|c| person(&c).ok()).collect();
    let q = text.to_lowercase();
    out.sort_by_key(|p| rank(p, &q));
    out.truncate(limit);
    Ok(out)
}

fn pairs(v: Vec<(String, String)>) -> Value {
    arr(v.into_iter().map(|(l, x)| obj(vec![("label", s(&l)), ("value", s(&x))])).collect())
}

pub(super) fn query(args: &[Value]) -> Value {
    let text = str_arg(args, 0);
    let limit = args.get(1).and_then(|v| v.as_number()).unwrap_or(50.0).clamp(1.0, 500.0) as usize;
    in_background(
        args.get(2),
        move || find(&text, limit),
        |r| match r {
            Ok(list) => obj(vec![
                (
                    "contacts",
                    arr(list
                        .into_iter()
                        .map(|p| {
                            obj(vec![
                                ("id", s(&p.id)),
                                ("name", s(&p.name)),
                                ("given", s(&p.given)),
                                ("family", s(&p.family)),
                                ("nickname", s(&p.nickname)),
                                ("org", s(&p.org)),
                                ("title", s(&p.title)),
                                ("emails", pairs(p.emails)),
                                ("phones", pairs(p.phones)),
                            ])
                        })
                        .collect()),
                ),
                ("error", s("")),
            ]),
            Err(e) => obj(vec![("contacts", arr(Vec::new())), ("error", s(&e))]),
        },
    );
    Value::Null
}
