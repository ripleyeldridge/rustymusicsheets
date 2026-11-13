use crate::{
    model::Paths,
    source::{ContentUri, DocumentAccess, DocumentRef, LocalDocuments, Metadata},
};
use android_activity::AndroidApp;
use anyhow::{bail, Context, Result};
use jni::{
    objects::{JObject, JString, JValue},
    JNIEnv, JavaVM,
};
use std::{fs::File, io::Read, os::fd::FromRawFd, path::PathBuf};

/// Holding AndroidApp keeps its borrowed Activity global reference valid.
pub struct AndroidDocuments {
    app: AndroidApp,
}

impl AndroidDocuments {
    pub fn new(app: AndroidApp) -> Self {
        Self { app }
    }

    pub(super) fn with_env<T>(
        &self,
        action: impl FnOnce(&mut JNIEnv<'_>, &JObject<'_>) -> Result<T>,
    ) -> Result<T> {
        // SAFETY: The app owns a live VM and borrowed global Activity reference.
        // JObject does not own/delete this reference; AndroidApp outlives the call.
        let vm = unsafe { JavaVM::from_raw(self.app.vm_as_ptr().cast()) }?;
        let mut env = vm.attach_current_thread()?;
        let activity = unsafe { JObject::from_raw(self.app.activity_as_ptr().cast()) };
        let result = env.with_local_frame(64, |env| action(env, &activity));
        // Rust handles errors here; do not leave a pending exception on a reused thread.
        if env.exception_check()? {
            env.exception_clear()?;
        }
        result
    }

    pub fn private_paths(&self) -> Result<Paths> {
        self.with_env(|env, activity| {
            let mut directory = |method| -> Result<PathBuf> {
                let file = env
                    .call_method(activity, method, "()Ljava/io/File;", &[])?
                    .l()?;
                let path = env
                    .call_method(file, "getAbsolutePath", "()Ljava/lang/String;", &[])?
                    .l()?;
                Ok(PathBuf::from(
                    string(env, path)?.context("Android private directory is missing")?,
                ))
            };
            let files = directory("getFilesDir")?;
            let cache = directory("getCacheDir")?;
            Paths::app_private(&files, &cache)
        })
    }

    /// Initial ACTION_VIEW only; a full SAF picker and onNewIntent bridge are future work.
    pub fn launch_document(&self) -> Result<Option<DocumentRef>> {
        self.with_env(|env, activity| {
            let intent = env
                .call_method(activity, "getIntent", "()Landroid/content/Intent;", &[])?
                .l()?;
            if intent.is_null() {
                return Ok(None);
            }
            let action = env
                .call_method(&intent, "getAction", "()Ljava/lang/String;", &[])?
                .l()?;
            if string(env, action)?.as_deref() != Some("android.intent.action.VIEW") {
                return Ok(None);
            }
            let data = env
                .call_method(&intent, "getDataString", "()Ljava/lang/String;", &[])?
                .l()?;
            string(env, data)?
                .map(|uri| ContentUri::parse(&uri).map(DocumentRef::Content))
                .transpose()
        })
    }
}

fn string(env: &mut JNIEnv<'_>, object: JObject<'_>) -> Result<Option<String>> {
    if object.is_null() {
        return Ok(None);
    }
    let value = JString::from(object);
    let text: String = env.get_string(&value)?.into();
    Ok(Some(text))
}

fn resolver<'a>(env: &mut JNIEnv<'a>, activity: &JObject<'_>) -> Result<JObject<'a>> {
    Ok(env
        .call_method(
            activity,
            "getContentResolver",
            "()Landroid/content/ContentResolver;",
            &[],
        )?
        .l()?)
}

fn uri<'a>(env: &mut JNIEnv<'a>, source: &ContentUri) -> Result<JObject<'a>> {
    let text = env.new_string(source.as_str())?;
    Ok(env
        .call_static_method(
            "android/net/Uri",
            "parse",
            "(Ljava/lang/String;)Landroid/net/Uri;",
            &[JValue::Object(&text)],
        )?
        .l()?)
}

impl DocumentAccess for AndroidDocuments {
    fn metadata(&self, source: &DocumentRef) -> Result<Metadata> {
        let DocumentRef::Content(content) = source else {
            return LocalDocuments.metadata(source);
        };
        self.with_env(|env, activity| {
            let resolver = resolver(env, activity)?;
            let uri = uri(env, content)?;
            let mime = env.call_method(&resolver, "getType", "(Landroid/net/Uri;)Ljava/lang/String;", &[JValue::Object(&uri)])?.l()?;
            let mime_type = string(env, mime)?;
            let projection = env.new_object_array(2, "java/lang/String", JObject::null())?;
            for (index, column) in ["_display_name", "_size"].into_iter().enumerate() {
                let column = env.new_string(column)?;
                env.set_object_array_element(&projection, index as i32, column)?;
            }
            let null = JObject::null();
            let cursor = env.call_method(
                &resolver, "query",
                "(Landroid/net/Uri;[Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;Ljava/lang/String;)Landroid/database/Cursor;",
                &[JValue::Object(&uri), JValue::Object(&projection), JValue::Object(&null), JValue::Object(&null), JValue::Object(&null)],
            )?.l()?;
            let mut metadata = Metadata { mime_type, ..Default::default() };
            if cursor.is_null() { return Ok(metadata); }
            let result = (|| -> Result<()> {
                if !env.call_method(&cursor, "moveToFirst", "()Z", &[])?.z()? { return Ok(()); }
                for column in ["_display_name", "_size"] {
                    let name = env.new_string(column)?;
                    let index = env.call_method(&cursor, "getColumnIndex", "(Ljava/lang/String;)I", &[JValue::Object(&name)])?.i()?;
                    if index < 0 || env.call_method(&cursor, "isNull", "(I)Z", &[JValue::Int(index)])?.z()? { continue; }
                    if column == "_display_name" {
                        let value = env.call_method(&cursor, "getString", "(I)Ljava/lang/String;", &[JValue::Int(index)])?.l()?;
                        metadata.display_name = string(env, value)?;
                    } else {
                        let value = env.call_method(&cursor, "getLong", "(I)J", &[JValue::Int(index)])?.j()?;
                        metadata.size = u64::try_from(value).ok();
                    }
                }
                Ok(())
            })();
            if env.exception_check()? { env.exception_clear()?; }
            let close = env.call_method(&cursor, "close", "()V", &[]);
            result?;
            close?;
            Ok(metadata)
        }).context("Cannot read document metadata; the provider or read grant may be unavailable")
    }

    fn open(&self, source: &DocumentRef) -> Result<Box<dyn Read + Send>> {
        let DocumentRef::Content(content) = source else {
            return LocalDocuments.open(source);
        };
        self.with_env(|env, activity| {
            let resolver = resolver(env, activity)?;
            let uri = uri(env, content)?;
            let mode = env.new_string("r")?;
            let descriptor = env
                .call_method(
                    &resolver,
                    "openFileDescriptor",
                    "(Landroid/net/Uri;Ljava/lang/String;)Landroid/os/ParcelFileDescriptor;",
                    &[JValue::Object(&uri), JValue::Object(&mode)],
                )?
                .l()?;
            if descriptor.is_null() {
                bail!("Document provider returned no readable descriptor");
            }
            let detached = env
                .call_method(&descriptor, "detachFd", "()I", &[])
                .and_then(|v| v.i());
            if detached.is_err() {
                if env.exception_check()? {
                    env.exception_clear()?;
                }
                let _ = env.call_method(&descriptor, "close", "()V", &[]);
            }
            let fd = detached?;
            if fd < 0 {
                bail!("Document provider returned an invalid descriptor");
            }
            // SAFETY: detachFd transfers exclusive ownership; File closes it exactly once.
            Ok(Box::new(unsafe { File::from_raw_fd(fd) }) as Box<dyn Read + Send>)
        })
        .context("Cannot open document; a valid Android read grant is required")
    }
}
