use wasm_bindgen::JsCast as _;

const SETTINGS_KEY: &str = "wordcraft.ui";
const APP_FOLDER: &str = "Ard";
const RECOVERY_PATH: &str = "Ard/recovery.docx";
const SESSION_STATE_PATH: &str = "Ard/session_state.json";
pub const DOCUMENTS_DIR: &str = "Ard/documents";

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Default)]
pub struct SessionState {
    pub current_doc: Option<String>,
    pub clean_exit: bool,
    pub last_active: f64,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DocumentMeta {
    pub name: String,
    pub path: String,
    pub mtime: f64,
    pub size: usize,
}

fn js_error(error: wasm_bindgen::JsValue) -> String {
    format!("{error:?}")
}

fn object(fields: &[(&str, wasm_bindgen::JsValue)]) -> Result<js_sys::Object, String> {
    let result = js_sys::Object::new();
    for (key, value) in fields {
        js_sys::Reflect::set(&result, &wasm_bindgen::JsValue::from_str(key), value).map_err(js_error)?;
    }
    Ok(result)
}

fn string(value: &str) -> wasm_bindgen::JsValue {
    wasm_bindgen::JsValue::from_str(value)
}

fn plugin(name: &str) -> Result<wasm_bindgen::JsValue, String> {
    let window = web_sys::window().ok_or_else(|| "no window".to_string())?;
    let capacitor = js_sys::Reflect::get(&window, &string("Capacitor")).map_err(js_error)?;
    let plugins = js_sys::Reflect::get(&capacitor, &string("Plugins")).map_err(js_error)?;
    let result = js_sys::Reflect::get(&plugins, &string(name)).map_err(js_error)?;
    if result.is_undefined() || result.is_null() {
        return Err(format!("Capacitor plugin {name} is unavailable"));
    }
    Ok(result)
}

fn invoke(name: &str, method: &str, options: &js_sys::Object) -> Result<js_sys::Promise, String> {
    let target = plugin(name)?;
    let member = js_sys::Reflect::get(&target, &string(method)).map_err(js_error)?;
    let function = member.dyn_into::<js_sys::Function>().map_err(js_error)?;
    let result = function.call1(&target, options).map_err(js_error)?;
    result.dyn_into::<js_sys::Promise>().map_err(js_error)
}

async fn call(name: &str, method: &str, fields: &[(&str, wasm_bindgen::JsValue)]) -> Result<wasm_bindgen::JsValue, String> {
    let options = object(fields)?;
    let promise = invoke(name, method, &options)?;
    wasm_bindgen_futures::JsFuture::from(promise).await.map_err(js_error)
}

pub fn is_android() -> bool {
    let Some(window) = web_sys::window() else { return false };
    let Ok(capacitor) = js_sys::Reflect::get(&window, &string("Capacitor")) else { return false };
    let Ok(get_platform) = js_sys::Reflect::get(&capacitor, &string("getPlatform")) else { return false };
    let Ok(get_platform) = get_platform.dyn_into::<js_sys::Function>() else { return false };
    get_platform.call0(&capacitor).ok().and_then(|v| v.as_string()).as_deref() == Some("android")
}

pub fn device_languages() -> Vec<String> {
    let Some(window) = web_sys::window() else { return Vec::new() };
    let Ok(navigator) = js_sys::Reflect::get(&window, &string("navigator")) else { return Vec::new() };
    let mut languages = Vec::new();
    if let Ok(value) = js_sys::Reflect::get(&navigator, &string("languages"))
        && let Ok(values) = value.dyn_into::<js_sys::Array>()
    {
        languages.extend(values.iter().filter_map(|value| value.as_string()));
    }
    if let Ok(value) = js_sys::Reflect::get(&navigator, &string("language"))
        && let Some(language) = value.as_string()
        && !languages.iter().any(|existing| existing == &language)
    {
        languages.push(language);
    }
    languages
}

pub async fn load_settings(android: bool) -> Result<Option<String>, String> {
    if android {
        let result = call("Preferences", "get", &[("key", string(SETTINGS_KEY))]).await?;
        return Ok(js_sys::Reflect::get(&result, &string("value")).map_err(js_error)?.as_string());
    }
    let window = web_sys::window().ok_or_else(|| "no window".to_string())?;
    let storage = window.local_storage().map_err(js_error)?.ok_or_else(|| "localStorage is unavailable".to_string())?;
    storage.get_item(SETTINGS_KEY).map_err(js_error)
}

pub async fn save_settings(android: bool, value: String) -> Result<(), String> {
    if android {
        call("Preferences", "set", &[("key", string(SETTINGS_KEY)), ("value", string(&value))]).await?;
        return Ok(());
    }
    let window = web_sys::window().ok_or_else(|| "no window".to_string())?;
    let storage = window.local_storage().map_err(js_error)?.ok_or_else(|| "localStorage is unavailable".to_string())?;
    storage.set_item(SETTINGS_KEY, &value).map_err(js_error)
}

pub async fn save_documents_file(path: &str, bytes: &[u8]) -> Result<(), String> {
    let relative = path.strip_prefix("Documents/").ok_or_else(|| "invalid Documents path".to_string())?;
    if !valid_relative_path(relative) {
        return Err("invalid Documents path".into());
    }
    let permissions = call("Filesystem", "checkPermissions", &[]).await?;
    let permission = js_sys::Reflect::get(&permissions, &string("publicStorage")).map_err(js_error)?.as_string();
    if permission.as_deref() != Some("granted") {
        let permissions = call("Filesystem", "requestPermissions", &[]).await?;
        let permission = js_sys::Reflect::get(&permissions, &string("publicStorage")).map_err(js_error)?.as_string();
        if permission.as_deref() == Some("denied") {
            return Err("Documents storage permission was denied".into());
        }
    }
    call(
        "Filesystem",
        "writeFile",
        &[
            ("path", string(relative)),
            ("data", string(&wordcraft_engine::cmd::insert::base64_encode(bytes))),
            ("directory", string("DOCUMENTS")),
            ("recursive", wasm_bindgen::JsValue::TRUE),
        ],
    )
    .await?;
    Ok(())
}

pub async fn write_private(path: &str, bytes: &[u8]) -> Result<(), String> {
    if !valid_relative_path(path) {
        return Err("invalid app-private file path".into());
    }
    call(
        "Filesystem",
        "writeFile",
        &[
            ("path", string(path)),
            ("data", string(&wordcraft_engine::cmd::insert::base64_encode(bytes))),
            ("directory", string("DATA")),
            ("recursive", wasm_bindgen::JsValue::TRUE),
        ],
    )
    .await?;
    Ok(())
}

pub async fn read_private(path: &str) -> Result<Vec<u8>, String> {
    if !valid_relative_path(path) {
        return Err("invalid app-private file path".into());
    }
    let result = call("Filesystem", "readFile", &[("path", string(path)), ("directory", string("DATA"))]).await?;
    let encoded = js_sys::Reflect::get(&result, &string("data")).map_err(js_error)?.as_string().ok_or_else(|| "invalid file data".to_string())?;
    wordcraft_engine::cmd::insert::base64_decode(&encoded).ok_or_else(|| "invalid base64 file data".to_string())
}

pub async fn remove_private(path: &str) -> Result<(), String> {
    if !valid_relative_path(path) {
        return Err("invalid app-private file path".into());
    }
    call("Filesystem", "deleteFile", &[("path", string(path)), ("directory", string("DATA"))]).await?;
    Ok(())
}

/// Atomic write to app's private persistent DATA directory:
/// 1. Write `bytes` to `{path}.tmp`
/// 2. If `{path}` already exists, rename it to `{path}.bak`
/// 3. Rename `{path}.tmp` to `{path}`
pub async fn write_private_atomic(path: &str, bytes: &[u8]) -> Result<(), String> {
    if !valid_relative_path(path) {
        return Err("invalid app-private file path".into());
    }
    let tmp_path = format!("{path}.tmp");
    let bak_path = format!("{path}.bak");

    // 1. Write content to temp file
    write_private(&tmp_path, bytes).await?;

    // 2. Backup existing file if present
    let _ = call(
        "Filesystem",
        "rename",
        &[("from", string(path)), ("to", string(&bak_path)), ("directory", string("DATA"))],
    )
    .await;

    // 3. Rename temp file to target
    call(
        "Filesystem",
        "rename",
        &[("from", string(&tmp_path)), ("to", string(path)), ("directory", string("DATA"))],
    )
    .await?;

    Ok(())
}

/// Reads a private file from DATA directory.
/// If primary file is missing or unreadable, automatically attempts to read the `{path}.bak` backup copy.
/// Returns `(file_bytes, is_restored_from_backup)`.
pub async fn read_private_with_bak(path: &str) -> Result<(Vec<u8>, bool), String> {
    if !valid_relative_path(path) {
        return Err("invalid app-private file path".into());
    }
    match read_private(path).await {
        Ok(bytes) if !bytes.is_empty() => Ok((bytes, false)),
        Err(primary_err) => {
            log::warn!("could not read primary file {path} ({primary_err}); checking for .bak backup");
            let bak_path = format!("{path}.bak");
            match read_private(&bak_path).await {
                Ok(bytes) if !bytes.is_empty() => {
                    log::info!("successfully read backup copy {bak_path}");
                    Ok((bytes, true))
                }
                Err(bak_err) => Err(format!("failed to read {path} ({primary_err}) and backup ({bak_err})")),
                Ok(_) => Err(format!("backup {bak_path} was empty")),
            }
        }
        Ok(_) => {
            let bak_path = format!("{path}.bak");
            match read_private(&bak_path).await {
                Ok(bytes) if !bytes.is_empty() => Ok((bytes, true)),
                _ => Err(format!("file {path} was empty")),
            }
        }
    }
}

pub async fn rename_private_document(old_name: &str, new_name: &str) -> Result<(), String> {
    let from_path = format!("{DOCUMENTS_DIR}/{old_name}");
    let to_path = format!("{DOCUMENTS_DIR}/{new_name}");
    call(
        "Filesystem",
        "rename",
        &[("from", string(&from_path)), ("to", string(&to_path)), ("directory", string("DATA"))],
    )
    .await?;

    // Also rename .bak if it exists
    let from_bak = format!("{from_path}.bak");
    let to_bak = format!("{to_path}.bak");
    let _ = call(
        "Filesystem",
        "rename",
        &[("from", string(&from_bak)), ("to", string(&to_bak)), ("directory", string("DATA"))],
    )
    .await;

    Ok(())
}

pub async fn delete_private_document(name: &str) -> Result<(), String> {
    let path = format!("{DOCUMENTS_DIR}/{name}");
    let _ = remove_private(&path).await;
    let _ = remove_private(&format!("{path}.bak")).await;
    let _ = remove_private(&format!("{path}.tmp")).await;
    Ok(())
}

pub async fn list_documents() -> Result<Vec<DocumentMeta>, String> {
    let result = match call("Filesystem", "readdir", &[("path", string(DOCUMENTS_DIR)), ("directory", string("DATA"))]).await {
        Ok(r) => r,
        Err(e) => {
            log::info!("documents directory does not exist or cannot be read: {e}");
            return Ok(Vec::new());
        }
    };

    let files_val = js_sys::Reflect::get(&result, &string("files")).map_err(js_error)?;
    let files_arr = files_val.dyn_into::<js_sys::Array>().map_err(js_error)?;
    let mut docs = Vec::new();

    for i in 0..files_arr.length() {
        let item = files_arr.get(i);
        let name = if let Some(s) = item.as_string() {
            s
        } else if let Ok(n) = js_sys::Reflect::get(&item, &string("name")) {
            n.as_string().unwrap_or_default()
        } else {
            continue;
        };

        if !name.ends_with(".docx") && !name.ends_with(".doc") {
            continue;
        }
        if name.ends_with(".tmp") || name.ends_with(".bak") {
            continue;
        }

        let mtime = js_sys::Reflect::get(&item, &string("mtime"))
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or_else(js_sys::Date::now);
        let size = js_sys::Reflect::get(&item, &string("size"))
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as usize;

        docs.push(DocumentMeta {
            path: format!("{DOCUMENTS_DIR}/{name}"),
            name,
            mtime,
            size,
        });
    }

    docs.sort_by(|a, b| b.mtime.partial_cmp(&a.mtime).unwrap_or(std::cmp::Ordering::Equal));
    Ok(docs)
}

pub async fn get_private_uri(path: &str) -> Result<String, String> {
    if !valid_relative_path(path) {
        return Err("invalid app-private file path".into());
    }
    let result = call("Filesystem", "getUri", &[("path", string(path)), ("directory", string("DATA"))]).await?;
    js_sys::Reflect::get(&result, &string("uri")).map_err(js_error)?.as_string().ok_or_else(|| "filesystem returned no share URI".into())
}

pub async fn share_file_uri(uri: &str, title: &str) -> Result<(), String> {
    let files = js_sys::Array::new();
    files.push(&string(uri));
    call("Share", "share", &[("title", string(title)), ("text", string(title)), ("files", files.into())]).await?;
    Ok(())
}

pub async fn load_session_state() -> SessionState {
    match read_private(SESSION_STATE_PATH).await {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => SessionState::default(),
    }
}

pub async fn save_session_state(state: &SessionState) -> Result<(), String> {
    let bytes = serde_json::to_vec(state).map_err(|e| e.to_string())?;
    write_private(SESSION_STATE_PATH, &bytes).await
}

pub fn app_add_listener<F>(event_name: &str, callback: F) -> Result<(), String>
where
    F: FnMut(wasm_bindgen::JsValue) + 'static,
{
    let target = plugin("App")?;
    let add_listener = js_sys::Reflect::get(&target, &string("addListener")).map_err(js_error)?;
    let add_listener_fn = add_listener.dyn_into::<js_sys::Function>().map_err(js_error)?;
    let closure = wasm_bindgen::closure::Closure::wrap(Box::new(callback) as Box<dyn FnMut(wasm_bindgen::JsValue)>);
    let js_fn = closure.as_ref().clone();
    closure.forget();
    add_listener_fn.call2(&target, &string(event_name), &js_fn).map_err(js_error)?;
    Ok(())
}

pub fn app_exit() -> Result<(), String> {
    let target = plugin("App")?;
    let exit_fn = js_sys::Reflect::get(&target, &string("exitApp")).map_err(js_error)?;
    let exit_fn = exit_fn.dyn_into::<js_sys::Function>().map_err(js_error)?;
    exit_fn.call0(&target).map_err(js_error)?;
    Ok(())
}

pub fn recovery_path() -> &'static str {
    RECOVERY_PATH
}

fn valid_relative_path(path: &str) -> bool {
    !path.is_empty() && !path.starts_with('/') && !path.contains('\\') && path.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
}
