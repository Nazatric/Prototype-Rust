//! First-launch **all-files-access** gate (Android 11 / API 30+).
//!
//! `MANAGE_EXTERNAL_STORAGE` is a special-access permission: declaring it
//! in the manifest grants nothing.  The user has to flip a switch in
//! Settings.  This module is the small JNI bridge that
//!
//! 1. asks the OS whether the switch is already on
//!    ([`all_files_access_granted`], `Environment.isExternalStorageManager`);
//! 2. opens the per-app Settings page when it is not
//!    ([`open_all_files_settings`], `ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION`).
//!
//! It deliberately uses the **raw JNI vtable** (`jni-sys`) instead of a
//! high-level wrapper: only four framework classes are touched, all of
//! them are visible from an attached native thread (the notorious
//! `FindClass` limitation applies to *application* classes only), and
//! the raw `...A` entry points take a plain `jvalue` array — no
//! variadics, no extra dependency negotiation.
//!
//! The two pointers come from `android_activity::AndroidApp`
//! (`vm_as_ptr()` / `activity_as_ptr()`); they live for the whole
//! process lifetime.
//!
//! # Safety
//!
//! JNI cannot be expressed in safe Rust.  This module is the second
//! documented exception to the workspace-wide `deny(unsafe_code)` rule
//! (the first is [`crate::activity`]); every unsafe block is confined to
//! the `#[cfg(target_os = "android")]` implementation below.

#![allow(unsafe_code)]

/// The application id declared in `[package.metadata.android]`.
///
/// It MUST match the generated manifest, otherwise the Settings deep
/// link opens a page for a package that does not exist.
pub const PACKAGE: &str = "com.nazatric.prototype_rs";

/// Raw JNI handles captured once by the NativeActivity entry point.
///
/// Stored as integers so the shared (desktop-compiled) [`crate::app::App`]
/// can hold them without pulling any Android-only types onto the host.
#[derive(Clone, Copy, Debug)]
pub struct PermissionBridge {
    /// `*mut JavaVM` (a pointer to the invocation-interface pointer).
    pub vm: usize,
    /// The `NativeActivity` global reference (`jobject`).
    pub activity: usize,
}

#[cfg(target_os = "android")]
pub use self::android::{all_files_access_granted, open_all_files_settings};

#[cfg(target_os = "android")]
mod android {
    use super::{PermissionBridge, PACKAGE};
    use jni_sys::{
        jmethodID, jobject, jvalue, JNIEnv, JNINativeInterface_, JNIInvokeInterface_, JavaVM,
        JNI_OK,
    };
    use std::ffi::{c_char, c_void};

    /// Every C string below is a NUL-terminated byte literal.
    fn c(bytes: &'static [u8]) -> *const c_char {
        bytes.as_ptr() as *const c_char
    }

    /// A borrowed `JNIEnv*` for the current thread.
    struct Jni {
        env: *mut JNIEnv,
    }

    impl Jni {
        /// The JNI function table (`JNIEnv` is a pointer-to-pointer).
        fn f(&self) -> &JNINativeInterface_ {
            // SAFETY: `env` came from `AttachCurrentThread` on a valid
            // JavaVM and stays valid for the process lifetime.
            unsafe { &**self.env }
        }

        fn class(&self, name: &'static [u8]) -> jobject {
            // SAFETY: framework classes only; `name` is NUL-terminated.
            unsafe { (self.f().v1_1.FindClass)(self.env, c(name)) }
        }

        fn exception_check(&self) -> bool {
            // SAFETY: JNI 1.1 call on a live env.  `ExceptionCheck` is
            // a 1.4 addition (absent from the v1_1 prefix), so probe with
            // `ExceptionOccurred` and drop the local reference it returns.
            unsafe {
                let exc = (self.f().v1_1.ExceptionOccurred)(self.env);
                if exc.is_null() {
                    false
                } else {
                    (self.f().v1_1.DeleteLocalRef)(self.env, exc);
                    true
                }
            }
        }

        fn clear_exception(&self) {
            // SAFETY: plain JNI 1.1 call on a live env.
            unsafe { (self.f().v1_1.ExceptionClear)(self.env) }
        }

        fn new_string(&self, s: &'static [u8]) -> jobject {
            // SAFETY: `s` is a NUL-terminated modified-UTF-8 literal.
            unsafe { (self.f().v1_1.NewStringUTF)(self.env, c(s)) }
        }

        fn static_method(&self, cls: jobject, name: &'static [u8], sig: &'static [u8]) -> jmethodID {
            // SAFETY: `cls` is a non-null local class ref from FindClass.
            unsafe { (self.f().v1_1.GetStaticMethodID)(self.env, cls, c(name), c(sig)) }
        }

        fn method(&self, cls: jobject, name: &'static [u8], sig: &'static [u8]) -> jmethodID {
            // SAFETY: `cls` is a non-null local class ref.
            unsafe { (self.f().v1_1.GetMethodID)(self.env, cls, c(name), c(sig)) }
        }
    }

    /// Attach the calling thread to the VM (cheap when already attached).
    fn attach(vm_raw: usize) -> Option<Jni> {
        if vm_raw == 0 {
            return None;
        }
        let vm = vm_raw as *mut JavaVM;
        // SAFETY: `vm_raw` is android-activity's `vm_as_ptr()`, a valid
        // `*mut JavaVM` for the lifetime of the process.
        unsafe {
            if vm.is_null() || (*vm).is_null() {
                return None;
            }
            let invoke: &JNIInvokeInterface_ = &**vm;
            let mut out: *mut c_void = std::ptr::null_mut();
            let rc = (invoke.v1_1.AttachCurrentThread)(vm, &mut out, std::ptr::null_mut());
            if rc != JNI_OK || out.is_null() {
                return None;
            }
            Some(Jni {
                env: out as *mut JNIEnv,
            })
        }
    }

    /// `android.os.Environment.isExternalStorageManager()` — the only
    /// reliable all-files-access probe on API 30+.
    pub fn all_files_access_granted(bridge: PermissionBridge) -> bool {
        let Some(j) = attach(bridge.vm) else {
            log::warn!("permission: could not attach to the Java VM");
            return false;
        };
        // SAFETY: all calls go through the attached env's function table.
        unsafe {
            let cls = j.class(b"android/os/Environment\0");
            if cls.is_null() {
                j.clear_exception();
                log::warn!("permission: android.os.Environment not found");
                return false;
            }
            let mid = j.static_method(
                cls,
                b"isExternalStorageManager\0",
                b"()Z\0",
            );
            if mid.is_null() {
                // API < 30: this method does not exist.  Clear the
                // NoSuchMethodError so it cannot poison later JNI calls.
                j.clear_exception();
                log::warn!("permission: isExternalStorageManager unavailable (API < 30)");
                return false;
            }
            let granted =
                (j.f().v1_1.CallStaticBooleanMethodA)(j.env, cls, mid, std::ptr::null());
            if j.exception_check() {
                j.clear_exception();
                return false;
            }
            granted
        }
    }

    /// Start one Settings `Intent`; returns false (and clears the
    /// pending Java exception) when the action is not resolvable.
    fn launch(
        j: &Jni,
        activity: jobject,
        action: &'static [u8],
        uri: jobject,
    ) -> bool {
        // SAFETY: framework classes only, all refs are non-null local
        // references owned by this attached thread.
        unsafe {
            let intent_cls = j.class(b"android/content/Intent\0");
            if intent_cls.is_null() {
                j.clear_exception();
                return false;
            }
            let action_str = j.new_string(action);
            if action_str.is_null() {
                j.clear_exception();
                return false;
            }

            let intent = if uri.is_null() {
                let ctor = j.method(
                    intent_cls,
                    b"<init>\0",
                    b"(Ljava/lang/String;)V\0",
                );
                if ctor.is_null() {
                    j.clear_exception();
                    return false;
                }
                let args = [jvalue { l: action_str }];
                (j.f().v1_1.NewObjectA)(j.env, intent_cls, ctor, args.as_ptr())
            } else {
                let ctor = j.method(
                    intent_cls,
                    b"<init>\0",
                    b"(Ljava/lang/String;Landroid/net/Uri;)V\0",
                );
                if ctor.is_null() {
                    j.clear_exception();
                    return false;
                }
                let args = [jvalue { l: action_str }, jvalue { l: uri }];
                (j.f().v1_1.NewObjectA)(j.env, intent_cls, ctor, args.as_ptr())
            };
            if j.exception_check() || intent.is_null() {
                j.clear_exception();
                return false;
            }

            let act_cls = (j.f().v1_1.GetObjectClass)(j.env, activity);
            if act_cls.is_null() {
                j.clear_exception();
                return false;
            }
            let start = j.method(
                act_cls,
                b"startActivity\0",
                b"(Landroid/content/Intent;)V\0",
            );
            if start.is_null() {
                j.clear_exception();
                return false;
            }
            let args = [jvalue { l: intent }];
            (j.f().v1_1.CallVoidMethodA)(j.env, activity, start, args.as_ptr());
            if j.exception_check() {
                j.clear_exception();
                return false;
            }
            true
        }
    }

    /// Open the per-app "Allow all files access" Settings page for
    /// [`PACKAGE`].  Falls back to the global all-files-access page on
    /// ROMs that do not implement the per-app deep link.
    pub fn open_all_files_settings(bridge: PermissionBridge) {
        if bridge.activity == 0 {
            log::warn!("permission: no NativeActivity reference");
            return;
        }
        let Some(j) = attach(bridge.vm) else {
            log::warn!("permission: could not attach to the Java VM");
            return;
        };
        let activity = bridge.activity as jobject;

        // SAFETY: framework classes only.
        unsafe {
            let uri_cls = j.class(b"android/net/Uri\0");
            let mut uri: jobject = std::ptr::null_mut();
            if !uri_cls.is_null() {
                let parse = j.static_method(
                    uri_cls,
                    b"parse\0",
                    b"(Ljava/lang/String;)Landroid/net/Uri;\0",
                );
                if !parse.is_null() {
                    let pkg = j.new_string(b"package:com.nazatric.prototype_rs\0");
                    if !pkg.is_null() {
                        let args = [jvalue { l: pkg }];
                        uri = (j.f().v1_1.CallStaticObjectMethodA)(
                            j.env,
                            uri_cls,
                            parse,
                            args.as_ptr(),
                        );
                        if j.exception_check() {
                            j.clear_exception();
                            uri = std::ptr::null_mut();
                        }
                    }
                } else {
                    j.clear_exception();
                }
            } else {
                j.clear_exception();
            }

            if launch(
                &j,
                activity,
                b"android.settings.MANAGE_APP_ALL_FILES_ACCESS_PERMISSION\0",
                uri,
            ) {
                log::info!("permission: opened all-files-access settings for {PACKAGE}");
                return;
            }
            log::warn!("permission: per-app settings page unavailable; trying global page");
            if launch(
                &j,
                activity,
                b"android.settings.MANAGE_ALL_FILES_ACCESS_PERMISSION\0",
                std::ptr::null_mut(),
            ) {
                log::info!("permission: opened global all-files-access settings");
            } else {
                log::error!("permission: no all-files-access settings activity found");
            }
        }
    }
}
