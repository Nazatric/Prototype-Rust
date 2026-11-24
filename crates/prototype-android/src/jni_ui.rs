//! JNI half of [`crate::platform_ui`] — toasts + the Android storage
//! grant UI (block 102).
//!
//! Compiled ONLY for `target_os = "android"` + the `android-activity`
//! feature (the same gate as [`crate::activity`]).  Everything here is
//! BEST-EFFORT by contract: every JNI call is fallible and every failure
//! is logged and swallowed — the grant flow must never take the app
//! down.  A missed toast just means the status colour carries the
//! message alone.
//!
//! # Patterns (all verified against jni 0.22.4 + android-activity 0.6.1)
//!
//! * Toast: the documented android-activity toast example (bind the
//!   types with `bind_java_type!`, hop to the Java main thread via
//!   `run_on_java_main_thread`, wrap the unowned activity global with
//!   `as_cast_raw`).
//! * All-files check: `Environment.isExternalStorageManager()` (API 30+)
//!   called on the CURRENT thread — a static, binder-backed read that
//!   needs no UI thread.
//! * Settings intent: `startActivity(Intent(ACTION_MANAGE_ALL_FILES_
//!   ACCESS_PERMISSION))` — opens the all-files LIST page (no package
//!   URI: one extra tap for the user, two fewer JNI calls, no
//!   ActivityNotFoundException risk on the toggle page).
//! * 8-10 dialog: `Activity.checkSelfPermission` + `requestPermissions`
//!   with `WRITE_EXTERNAL_STORAGE` (manifest-declared with
//!   max_sdk_version 29).
//!
//! The runtime API level comes from [`AndroidApp::sdk_version`] (the
//! glue reads `ro.build.version.sdk` — the REAL runtime level, no JNI).
//!
//! # Safety
//!
//! JNI calls cannot be expressed in safe Rust.  This module (block 102)
//! joins `activity` as a documented module-level unsafe exception to
//! the crate-wide deny — every `unsafe` block here is one of the two
//! doc-verified casts: the JavaVM pointer and the unowned activity
//! global (`as_cast_raw`, which BORROWS the global rather than owning
//! it, exactly as the android-activity 0.6.1 examples do).

#![allow(unsafe_code)]

use android_activity::AndroidApp;
use jni::{jni_sig, jni_str};
use jni::objects::{JObject, JString, JValue};
use jni::refs::Global;

use crate::platform_ui::{storage_plan, PlatformUi, StorageState, StoragePlan};

jni::bind_java_type! { Context => "android.content.Context" }
jni::bind_java_type! {
    Activity => "android.app.Activity",
    type_map {
        Context => "android.content.Context",
    },
    is_instance_of {
        context: Context
    },
}

jni::bind_java_type! {
    Toast => "android.widget.Toast",
    type_map {
        Context => "android.content.Context",
    },
    methods {
        static fn make_text(context: Context, text: JCharSequence, duration: i32) -> Toast,
        fn show(),
    }
}

/// Toast duration — `Toast.LENGTH_LONG` (the status flow is the point).
const TOAST_LONG: i32 = 1;

/// `PERMISSION_GRANTED` — `Activity.checkSelfPermission`'s OK value.
const PERMISSION_GRANTED: i32 = 0;
/// Our `requestPermissions` callback code (arbitrary, documented here).
const WRITE_REQUEST_CODE: i32 = 10201;

/// Toast text shown while the all-files page is up (Android 11+).
pub(crate) const ALL_FILES_PROMPT: &str =
    "Prototype Rust needs All files access — enable it, then press Back";

/// The JNI implementation of [`PlatformUi`].
pub struct AndroidUi {
    app: AndroidApp,
}

impl AndroidUi {
    /// Build the UI from the app handle (clone it before winit consumes
    /// the original — `AndroidApp` is `Clone`).
    pub fn new(app: AndroidApp) -> Self {
        Self { app }
    }

    /// Toast via the documented pattern: hop to the Java main thread,
    /// attach, wrap the unowned activity global, makeText + show.
    fn send_toast(&self, msg: &str) {
        let app = self.app.clone();
        let msg = msg.to_string();
        self.app.run_on_java_main_thread(Box::new(move || {
            // SAFETY: vm_as_ptr is the runtime's JavaVM, valid for the
            // app's lifetime (the docs' own toast example shape).
            let jvm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr() as _) };
            if let Err(err) = jvm.attach_current_thread(|env| -> jni::errors::Result<()> {
                let activity: jni::sys::jobject = app.activity_as_ptr() as _;
                // SAFETY: as_cast_raw BORROWS the unowned global (the
                // docs' own pattern — wrapping it as an owning Global
                // would delete it on drop).
                let activity = unsafe { env.as_cast_raw::<Global<Activity>>(&activity)? };
                let message = JString::new(env, &msg)?;
                let toast = Toast::make_text(env, activity.as_ref(), &message, TOAST_LONG)?;
                toast.show(env)?;
                Ok(())
            }) {
                log::warn!("platform ui: toast failed (non-fatal): {err:?}");
            }
        }));
    }

    /// `Environment.isExternalStorageManager()` on the current thread.
    fn all_files_granted(&self) -> bool {
        let app = self.app.clone();
        // SAFETY: the app's JavaVM pointer (docs' pattern).
        let jvm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr() as _) };
        jvm.attach_current_thread(|env| -> jni::errors::Result<bool> {
            let env_cls = env.find_class(jni_str!("android/os/Environment"))?;
            match env.call_static_method(
                &env_cls,
                jni_str!("isExternalStorageManager"),
                jni_sig!("()Z"),
                &[],
            ) {
                Ok(res) => res.z(),
                // A pending Java exception would poison every later JNI
                // call on this thread — clear it before reporting the
                // miss (fail-closed toward asking = the safe side).
                Err(e) => {
                    let _ = env.exception_clear();
                    Err(e)
                }
            }
        })
        .unwrap_or_else(|e| {
            log::warn!("platform ui: all-files check failed (non-fatal): {e:?}");
            false
        })
    }

    /// The classic `WRITE_EXTERNAL_STORAGE` runtime state (Android 8-10).
    fn write_permission_granted(&self) -> bool {
        let app = self.app.clone();
        // SAFETY: the app's JavaVM pointer (docs' pattern).
        let jvm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr() as _) };
        jvm.attach_current_thread(|env| -> jni::errors::Result<bool> {
            let activity: jni::sys::jobject = app.activity_as_ptr() as _;
            let activity = unsafe { env.as_cast_raw::<Global<Activity>>(&activity)? };
            let perm = env.new_string("android.permission.WRITE_EXTERNAL_STORAGE")?;
            let res = env.call_method(
                activity.as_ref(),
                jni_str!("checkSelfPermission"),
                jni_sig!("(Ljava/lang/String;)I"),
                &[JValue::Object(&perm)],
            )?;
            Ok(res.i()? == PERMISSION_GRANTED)
        })
        .unwrap_or_else(|e| {
            log::warn!("platform ui: permission check failed (non-fatal): {e:?}");
            false
        })
    }

    /// Open the Android 11+ all-files-access settings.  Block 104: try
    /// the APP-SPECIFIC toggle page first (`ACTION_MANAGE_APP_ALL_FILES_
    /// ACCESS_PERMISSION` + a `package:` URI — ONE tap for the user);
    /// ANY failure (ActivityNotFoundException on some OEM builds, a JNI
    /// hiccup) falls back to the plain list page, which every API 30+
    /// device has.  The pending Java exception is cleared before the
    /// fallback (a left-pending exception would poison every later JNI
    /// call on the thread).
    fn open_all_files_settings(&self) {
        let app = self.app.clone();
        self.app.run_on_java_main_thread(Box::new(move || {
            // SAFETY: the app's JavaVM pointer (docs' pattern).
            let jvm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr() as _) };
            if let Err(err) = jvm.attach_current_thread(|env| -> jni::errors::Result<()> {
                let activity: jni::sys::jobject = app.activity_as_ptr() as _;
                // SAFETY: borrows the unowned global (docs' pattern).
                let activity = unsafe { env.as_cast_raw::<Global<Activity>>(&activity)? };
                let intent_cls = env.find_class(jni_str!("android/content/Intent"))?;
                let uri_cls = env.find_class(jni_str!("android/net/Uri"))?;

                // Attempt 1: the APP-SPECIFIC all-files toggle page.
                // Uri.fromParts("package", <pkg>, null) IS the
                // `package:<pkg>` URI without any Java-side string
                // concat, and the package name goes Java→Java (the
                // getPackageName result straight into Uri.fromParts —
                // no Rust round-trip, no MUTF8 dance).
                let direct = (|| -> jni::errors::Result<()> {
                    let action = env.new_string(
                        "android.settings.MANAGE_APP_ALL_FILES_ACCESS_PERMISSION",
                    )?;
                    let package = env
                        .call_method(
                            activity.as_ref(),
                            jni_str!("getPackageName"),
                            jni_sig!("()Ljava/lang/String;"),
                            &[],
                        )?
                        .l()?;
                    let scheme = env.new_string("package")?;
                    let uri = env
                        .call_static_method(
                            &uri_cls,
                            jni_str!("fromParts"),
                            jni_sig!("(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)Landroid/net/Uri;"),
                            &[JValue::Object(&scheme), JValue::Object(&package), JValue::Object(&JObject::null())],
                        )?
                        .l()?;
                    let intent = env.new_object(
                        &intent_cls,
                        jni_sig!("(Ljava/lang/String;)V"),
                        &[JValue::Object(&action)],
                    )?;
                    env.call_method(
                        &intent,
                        jni_str!("setData"),
                        jni_sig!("(Landroid/net/Uri;)Landroid/content/Intent;"),
                        &[JValue::Object(&uri)],
                    )?;
                    env.call_method(
                        activity.as_ref(),
                        jni_str!("startActivity"),
                        jni_sig!("(Landroid/content/Intent;)V"),
                        &[JValue::Object(&intent)],
                    )?;
                    Ok(())
                })();
                if direct.is_err() {
                    // Clear the pending exception (ActivityNotFound on
                    // some OEM builds) before the fallback.
                    let _ = env.exception_clear();
                    // Attempt 2: the all-files LIST page (always exists
                    // on API 30+; the block-102 behaviour).
                    let action = env.new_string(
                        "android.settings.MANAGE_ALL_FILES_ACCESS_PERMISSION",
                    )?;
                    let intent = env.new_object(
                        &intent_cls,
                        jni_sig!("(Ljava/lang/String;)V"),
                        &[JValue::Object(&action)],
                    )?;
                    env.call_method(
                        activity.as_ref(),
                        jni_str!("startActivity"),
                        jni_sig!("(Landroid/content/Intent;)V"),
                        &[JValue::Object(&intent)],
                    )?;
                }
                Ok(())
            }) {
                log::warn!("platform ui: all-files settings failed (non-fatal): {err:?}");
            }
        }));
    }

    /// Fire the classic runtime dialog (Android 8-10, main thread).
    fn request_write_dialog(&self) {
        let app = self.app.clone();
        self.app.run_on_java_main_thread(Box::new(move || {
            // SAFETY: the app's JavaVM pointer (docs' pattern).
            let jvm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr() as _) };
            if let Err(err) = jvm.attach_current_thread(|env| -> jni::errors::Result<()> {
                let activity: jni::sys::jobject = app.activity_as_ptr() as _;
                // SAFETY: borrows the unowned global (docs' pattern).
                let activity = unsafe { env.as_cast_raw::<Global<Activity>>(&activity)? };
                let perm = env.new_string("android.permission.WRITE_EXTERNAL_STORAGE")?;
                let str_cls = env.find_class(jni_str!("java/lang/String"))?;
                let perms = env.new_object_array(1, &str_cls, &perm)?;
                env.call_method(
                    activity.as_ref(),
                    jni_str!("requestPermissions"),
                    jni_sig!("([Ljava/lang/String;I)V"),
                    &[JValue::Object(&perms), JValue::Int(WRITE_REQUEST_CODE)],
                )?;
                Ok(())
            }) {
                log::warn!("platform ui: write-permission dialog failed (non-fatal): {err:?}");
            }
        }));
    }
}

impl PlatformUi for AndroidUi {
    /// Block 106: immersive fullscreen + screen-on, applied on EVERY
    /// resume (returning from Settings or a system dialog re-applies
    /// it — the bars never come back permanently).  Landscape itself is
    /// locked in the manifest (`sensorLandscape`); this hides both
    /// system bars and keeps the display awake during play.
    fn apply_window_presentation(&self) {
        let app = self.app.clone();
        self.app.run_on_java_main_thread(Box::new(move || {
            // SAFETY: the app's JavaVM pointer (docs' pattern).
            let jvm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr() as _) };
            if let Err(err) = jvm.attach_current_thread(|env| -> jni::errors::Result<()> {
                let activity: jni::sys::jobject = app.activity_as_ptr() as _;
                // SAFETY: borrows the unowned global (docs' pattern).
                let activity = unsafe { env.as_cast_raw::<Global<Activity>>(&activity)? };
                let window = env
                    .call_method(
                        activity.as_ref(),
                        jni_str!("getWindow"),
                        jni_sig!("()Landroid/view/Window;"),
                        &[],
                    )?
                    .l()?;
                // FLAG_KEEP_SCREEN_ON (0x80) — a streamed open world must
                // not let the display sleep mid-play.
                env.call_method(
                    &window,
                    jni_str!("addFlags"),
                    jni_sig!("(I)V"),
                    &[JValue::Int(0x80)],
                )?;
                if AndroidApp::sdk_version() >= 30 {
                    // API 30+: the insets controller hides BOTH bars and
                    // lets a swipe reveal them transiently.
                    env.call_method(
                        &window,
                        jni_str!("setDecorFitsSystemWindows"),
                        jni_sig!("(Z)V"),
                        &[JValue::Bool(false)],
                    )?;
                    let controller = env
                        .call_method(
                            &window,
                            jni_str!("getInsetsController"),
                            jni_sig!("()Landroid/view/WindowInsetsController;"),
                            &[],
                        )?
                        .l()?;
                    let wi_type =
                        env.find_class(jni_str!("android/view/WindowInsets$Type"))?;
                    let sb = env
                        .call_static_method(
                            &wi_type,
                            jni_str!("statusBars"),
                            jni_sig!("()I"),
                            &[],
                        )?
                        .i()?;
                    let nb = env
                        .call_static_method(
                            &wi_type,
                            jni_str!("navigationBars"),
                            jni_sig!("()I"),
                            &[],
                        )?
                        .i()?;
                    env.call_method(
                        &controller,
                        jni_str!("hide"),
                        jni_sig!("(I)V"),
                        &[JValue::Int(sb | nb)],
                    )?;
                    // BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE (= 2).
                    env.call_method(
                        &controller,
                        jni_str!("setSystemBarsBehavior"),
                        jni_sig!("(I)V"),
                        &[JValue::Int(2)],
                    )?;
                } else {
                    // API 26-29: the classic immersive-sticky flags.
                    // HIDE_NAVIGATION(0x2) | FULLSCREEN(0x4) |
                    // LAYOUT_STABLE(0x100) | LAYOUT_HIDE_NAVIGATION(0x200)
                    // | LAYOUT_FULLSCREEN(0x400) | IMMERSIVE_STICKY(0x1000)
                    let decor = env
                        .call_method(
                            &window,
                            jni_str!("getDecorView"),
                            jni_sig!("()Landroid/view/View;"),
                            &[],
                        )?
                        .l()?;
                    env.call_method(
                        &decor,
                        jni_str!("setSystemUiVisibility"),
                        jni_sig!("(I)V"),
                        &[JValue::Int(0x2 | 0x4 | 0x100 | 0x200 | 0x400 | 0x1000)],
                    )?;
                }
                Ok(())
            }) {
                log::warn!("platform ui: immersive window failed (non-fatal): {err:?}");
            }
        }));
    }

    fn ensure_storage(&mut self) -> StorageState {
        let api = AndroidApp::sdk_version();
        // Block 104 FIX (the user's yellow screen): block 103 shipped a
        // LITERAL `true` as the granted flag, so the app reported
        // Granted on a fresh install and never opened Settings.  The
        // flag must come from the REAL all-files check — and only on
        // API 30+ (the method does not exist below it; a pre-30 call
        // would leave a pending Java exception on the thread).
        let granted = if api >= 30 { self.all_files_granted() } else { false };
        match storage_plan(api, granted) {
            StoragePlan::Granted => StorageState::Granted,
            StoragePlan::AllFilesSettings => {
                if self.all_files_granted() {
                    StorageState::Granted
                } else {
                    log::info!("platform ui: API {api} — requesting all-files access");
                    self.send_toast(ALL_FILES_PROMPT);
                    self.open_all_files_settings();
                    StorageState::Denied
                }
            }
            StoragePlan::RuntimeDialog => {
                if self.write_permission_granted() {
                    StorageState::Granted
                } else {
                    log::info!("platform ui: API {api} — requesting WRITE_EXTERNAL_STORAGE");
                    self.send_toast("Allow storage access to copy game files");
                    self.request_write_dialog();
                    StorageState::Denied
                }
            }
        }
    }

    fn toast(&self, message: &str) {
        self.send_toast(message);
    }
}
