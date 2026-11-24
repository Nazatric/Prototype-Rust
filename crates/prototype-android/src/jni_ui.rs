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
use jni::objects::{JString, JValue};
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
            let res = env.call_static_method(
                &env_cls,
                jni_str!("isExternalStorageManager"),
                jni_sig!("()Z"),
                &[],
            )?;
            res.z()
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

    /// Open the Android 11+ all-files-access settings page (main thread).
    fn open_all_files_settings(&self) {
        let app = self.app.clone();
        self.app.run_on_java_main_thread(Box::new(move || {
            // SAFETY: the app's JavaVM pointer (docs' pattern).
            let jvm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr() as _) };
            if let Err(err) = jvm.attach_current_thread(|env| -> jni::errors::Result<()> {
                let activity: jni::sys::jobject = app.activity_as_ptr() as _;
                // SAFETY: borrows the unowned global (docs' pattern).
                let activity = unsafe { env.as_cast_raw::<Global<Activity>>(&activity)? };
                let action = env.new_string(
                    "android.settings.MANAGE_ALL_FILES_ACCESS_PERMISSION",
                )?;
                let intent_cls = env.find_class(jni_str!("android/content/Intent"))?;
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
    fn ensure_storage(&mut self) -> StorageState {
        let api = AndroidApp::sdk_version();
        match storage_plan(api, true) {
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
