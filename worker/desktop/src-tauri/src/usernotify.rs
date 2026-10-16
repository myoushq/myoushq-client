//! A Mac's notifications through UserNotifications, as myous for Mac
//! posts them (`App.m`, setupNotifications and notifyOnce): a question
//! from a worker comes with Allow and Refuse (the category "approval"),
//! which answer it without the window; a click on any notification
//! brings up the window; a question's notification goes away once the
//! question is answered elsewhere or has timed out.
//!
//! UNUserNotificationCenter aborts the process when it isn't inside an
//! app bundle (`cargo run`, the test binary), so none of this runs there:
//! [`setup`] says no and the app keeps the notification plugin's plain
//! notifications. Inside the bundle the plugin's can't stand in: usernoted
//! refuses a process that mixes them with these. So while the owner hasn't
//! allowed notifications (System Settings › Notifications › myous) there
//! are none, and the app brings up the window for a question, as before;
//! also for the first question after the owner turned them off, which the
//! app still posts (it knew them allowed) and macOS refuses: the window
//! hears of it (`notify-failed`) and comes up with the question.

use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

use block2::{DynBlock, RcBlock};
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, ClassType};
use objc2_foundation::{NSArray, NSBundle, NSError, NSSet, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification,
    UNNotificationAction, UNNotificationActionOptions, UNNotificationCategory,
    UNNotificationCategoryOptions, UNNotificationPresentationOptions, UNNotificationRequest,
    UNNotificationResponse, UNNotificationSettings, UNNotificationSound, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};

/// What the owner did with a notification.
#[derive(Debug, PartialEq, Eq)]
pub enum Response {
    /// Allow or Refuse on a question's notification: the request id and
    /// "allow" or "refuse".
    Answer { id: String, verdict: &'static str },
    /// A click on the notification itself.
    Open,
    /// A question's notification macOS didn't show (the owner turned
    /// notifications off since the app last asked): its worker folder and
    /// request id, so the window comes up with the question instead.
    NotShown { home: String, id: String },
}

const CATEGORY: &str = "approval";

/// Whether the owner allows the app's notifications, as last heard.
const UNKNOWN: u8 = 0;
const ALLOWED: u8 = 1;
const DENIED: u8 = 2;
static ALLOWANCE: AtomicU8 = AtomicU8::new(UNKNOWN);
/// Set once [`setup`] has found an app bundle and taken the centre.
static ACTIVE: AtomicBool = AtomicBool::new(false);
static HANDLER: OnceLock<Box<dyn Fn(Response) + Send + Sync>> = OnceLock::new();
/// The questions shown, as (worker folder, request id), until answered or gone.
static ASKED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
/// A counter for the other notifications' identifiers.
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Whether this process is an app (myous.app/Contents/MacOS/...) with a
/// bundle identifier, which UserNotifications and SMAppService need. A
/// program run from target/ is not, even though Tauri gives it an
/// Info.plist of its own.
pub fn in_app_bundle() -> bool {
    static IN: OnceLock<bool> = OnceLock::new();
    *IN.get_or_init(|| {
        let bundle = NSBundle::mainBundle();
        bundle.bundleIdentifier().is_some() && bundle.bundlePath().to_string().ends_with(".app")
    })
}

/// Whether [`setup`] took the notification centre (the app, from its bundle).
pub fn active() -> bool {
    ACTIVE.load(Ordering::SeqCst)
}

/// The identifier of a question's notification, as myous for Mac names it.
pub fn ask_identifier(id: &str) -> String {
    format!("ask-{id}")
}

/// What a press means: Allow or Refuse on a question, or a click on the
/// notification (anything else, including a click on a question).
pub fn response_for(identifier: &str, action: &str) -> Response {
    match (identifier.strip_prefix("ask-"), action) {
        (Some(id), "allow") if !id.is_empty() => Response::Answer {
            id: id.to_string(),
            verdict: "allow",
        },
        (Some(id), "refuse") if !id.is_empty() => Response::Answer {
            id: id.to_string(),
            verdict: "refuse",
        },
        _ => Response::Open,
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and this class
    // doesn't implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "MyousNotificationDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        // While the app is in front, show the banner anyway (as myous for Mac).
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            handler: &DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            handler.call((UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::Sound,));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            handler: &DynBlock<dyn Fn()>,
        ) {
            let identifier = response.notification().request().identifier().to_string();
            let action = response.actionIdentifier().to_string();
            let r = response_for(&identifier, &action);
            if let Response::Answer { id, .. } = &r {
                forget(|_, asked| asked == id);
            }
            if let Some(h) = HANDLER.get() {
                h(r);
            }
            handler.call(());
        }
    }
);

fn center() -> Retained<UNUserNotificationCenter> {
    UNUserNotificationCenter::currentNotificationCenter()
}

fn allowed(status: UNAuthorizationStatus) -> bool {
    status == UNAuthorizationStatus::Authorized
        || status == UNAuthorizationStatus::Provisional
        || status == UNAuthorizationStatus::Ephemeral
}

/// Takes the notification centre: the delegate, the "approval" category
/// with Allow and Refuse, and the owner's permission, asked for once
/// (macOS asks only the first time; after that it answers with what the
/// owner chose). `handler` gets what the owner did.
/// Returns false outside an app bundle, where nothing was done.
pub fn setup(handler: impl Fn(Response) + Send + Sync + 'static) -> bool {
    if !in_app_bundle() {
        return false;
    }
    if ACTIVE.swap(true, Ordering::SeqCst) {
        return true;
    }
    let _ = HANDLER.set(Box::new(handler));
    let c = center();
    let delegate: Retained<Delegate> = unsafe { msg_send![Delegate::class(), new] };
    c.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    // The centre holds its delegate weakly; this one lives as long as the app.
    std::mem::forget(delegate);

    let allow = UNNotificationAction::actionWithIdentifier_title_options(
        &NSString::from_str("allow"),
        &NSString::from_str("Allow"),
        UNNotificationActionOptions::empty(),
    );
    let refuse = UNNotificationAction::actionWithIdentifier_title_options(
        &NSString::from_str("refuse"),
        &NSString::from_str("Refuse"),
        UNNotificationActionOptions::Destructive,
    );
    let category = UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
        &NSString::from_str(CATEGORY),
        &NSArray::from_retained_slice(&[allow, refuse]),
        &NSArray::new(),
        UNNotificationCategoryOptions::empty(),
    );
    c.setNotificationCategories(&NSSet::from_retained_slice(&[category]));

    let done = RcBlock::new(|granted: Bool, error: *mut NSError| {
        let before = ALLOWANCE.swap(
            if granted.as_bool() { ALLOWED } else { DENIED },
            Ordering::SeqCst,
        );
        if !granted.as_bool() && before != DENIED {
            // SAFETY: null or a valid error for the call.
            let why = match unsafe { error.as_ref() } {
                Some(e) => format!(" ({}, code {})", e.localizedDescription(), e.code()),
                None => String::new(),
            };
            crate::files::append_app_log(&format!(
                "notifications: not allowed{why} (System Settings › Notifications › myous); a question brings up the window",
            ));
        }
    });
    c.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
        &done,
    );
    true
}

/// Asks the centre whether notifications are still allowed (the owner
/// can change it in System Settings at any time); the answer counts from
/// the next notification on.
fn recheck() {
    let done = RcBlock::new(|settings: NonNull<UNNotificationSettings>| {
        // SAFETY: the centre hands a valid settings object for the call.
        let status = unsafe { settings.as_ref() }.authorizationStatus();
        ALLOWANCE.store(
            if allowed(status) { ALLOWED } else { DENIED },
            Ordering::SeqCst,
        );
    });
    center().getNotificationSettingsWithCompletionHandler(&done);
}

/// Shows a notification through the centre: a question (`ask` is its
/// worker folder and request id) with Allow and Refuse, anything else
/// plain. `sound` plays the default sound. Returns false when it can't
/// (outside an app bundle, or notifications not allowed), and the caller
/// brings up the window for a question (and, outside an app bundle, uses
/// the plugin).
pub fn post(title: &str, body: &str, sound: bool, ask: Option<(&str, &str)>) -> bool {
    if !ACTIVE.load(Ordering::SeqCst) {
        return false;
    }
    let ok = ALLOWANCE.load(Ordering::SeqCst) == ALLOWED;
    recheck();
    if !ok {
        return false;
    }
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    if sound {
        content.setSound(Some(&UNNotificationSound::defaultSound()));
    }
    let asked = ask.map(|(home, id)| (home.to_string(), id.to_string()));
    let identifier = match ask {
        Some((home, id)) => {
            content.setCategoryIdentifier(&NSString::from_str(CATEGORY));
            if let Ok(mut asked) = ASKED.lock() {
                asked.retain(|(_, i)| i != id);
                asked.push((home.to_string(), id.to_string()));
            }
            ask_identifier(id)
        }
        None => format!("note-{}", NEXT.fetch_add(1, Ordering::SeqCst)),
    };
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(&identifier),
        &content,
        None,
    );
    let done = RcBlock::new(move |error: *mut NSError| {
        // SAFETY: null or a valid error for the call.
        let Some(e) = (unsafe { error.as_ref() }) else {
            return;
        };
        crate::files::append_app_log(&format!(
            "notification: {} (code {})",
            e.localizedDescription(),
            e.code()
        ));
        if let Some((home, id)) = asked.clone() {
            not_shown(home, id);
        }
    });
    center().addNotificationRequest_withCompletionHandler(&request, Some(&done));
    true
}

/// A question's notification macOS didn't show (notifications turned off
/// since the app last asked, so the allowance it went by was out of
/// date): the question must not wait unseen, so the window comes up with
/// it (`Response::NotShown`), and the next question goes there at once.
fn not_shown(home: String, id: String) {
    ALLOWANCE.store(DENIED, Ordering::SeqCst);
    forget(|_, i| i == id);
    if let Some(h) = HANDLER.get() {
        h(Response::NotShown { home, id });
    }
}

/// Takes away the notifications of these requests (answered in the
/// window, or gone).
pub fn withdraw(ids: &[String]) {
    if !ACTIVE.load(Ordering::SeqCst) || ids.is_empty() {
        return;
    }
    let names: Vec<Retained<NSString>> = ids
        .iter()
        .map(|id| NSString::from_str(&ask_identifier(id)))
        .collect();
    let names = NSArray::from_retained_slice(&names);
    let c = center();
    c.removeDeliveredNotificationsWithIdentifiers(&names);
    c.removePendingNotificationRequestsWithIdentifiers(&names);
}

/// Drops the questions `gone` picks (by worker folder and request id)
/// from those shown, and returns their request ids.
fn forget(gone: impl Fn(&str, &str) -> bool) -> Vec<String> {
    let Ok(mut asked) = ASKED.lock() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    asked.retain(|(home, id)| {
        let g = gone(home, id);
        if g {
            out.push(id.clone());
        }
        !g
    });
    out
}

/// A question was answered in the window: its notification goes.
pub fn answered(id: &str) {
    withdraw(&forget(|_, asked| asked == id));
}

/// Takes away the notification of every question shown that `pending`
/// says no longer waits (answered elsewhere, or timed out: the worker
/// removed it).
pub fn withdraw_gone(pending: impl Fn(&str, &str) -> bool) {
    if !ACTIVE.load(Ordering::SeqCst) {
        return;
    }
    withdraw(&forget(|home, id| !pending(home, id)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presses_answer_the_question() {
        assert_eq!(
            response_for("ask-r5", "allow"),
            Response::Answer {
                id: "r5".into(),
                verdict: "allow"
            }
        );
        assert_eq!(
            response_for("ask-r-6_x", "refuse"),
            Response::Answer {
                id: "r-6_x".into(),
                verdict: "refuse"
            }
        );
        // A click on the notification, on a question or any other.
        assert_eq!(
            response_for("ask-r5", "com.apple.UNNotificationDefaultActionIdentifier"),
            Response::Open
        );
        assert_eq!(response_for("note-3", "allow"), Response::Open);
        assert_eq!(response_for("ask-", "allow"), Response::Open);
        assert_eq!(ask_identifier("r5"), "ask-r5");
    }

    /// cargo test runs outside an app bundle: nothing touches the centre
    /// (it would abort the process), and posting says no.
    #[test]
    fn outside_an_app_nothing_is_posted() {
        assert!(!in_app_bundle());
        assert!(!setup(|_| {}));
        assert!(!active());
        assert!(!post("t", "b", true, Some(("/tmp/w", "r1"))));
        withdraw(&["r1".into()]);
        withdraw_gone(|_, _| false);
    }

    #[test]
    fn answered_and_gone_questions_are_forgotten() {
        ASKED.lock().unwrap().extend([
            ("/w1".to_string(), "a1".to_string()),
            ("/w1".to_string(), "a2".to_string()),
            ("/w2".to_string(), "a3".to_string()),
        ]);
        assert_eq!(forget(|_, id| id == "a2"), vec!["a2".to_string()]);
        assert_eq!(forget(|home, _| home == "/w2"), vec!["a3".to_string()]);
        assert_eq!(forget(|_, _| true), vec!["a1".to_string()]);
        assert!(ASKED.lock().unwrap().is_empty());
        // One macOS didn't show: forgotten, and the next goes to the window.
        ASKED
            .lock()
            .unwrap()
            .push(("/w3".to_string(), "a4".to_string()));
        ALLOWANCE.store(ALLOWED, Ordering::SeqCst);
        not_shown("/w3".into(), "a4".into());
        assert!(ASKED.lock().unwrap().is_empty());
        assert_eq!(ALLOWANCE.load(Ordering::SeqCst), DENIED);
    }
}
