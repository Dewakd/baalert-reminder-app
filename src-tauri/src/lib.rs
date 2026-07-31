#[cfg(not(target_os = "macos"))]
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL_SAFE_NO_PAD;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
#[cfg(target_os = "macos")]
use std::{
    ffi::CString,
    sync::atomic::{AtomicBool, Ordering},
    thread::JoinHandle,
    time::Instant,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
#[cfg(not(target_os = "macos"))]
use tauri::webview::WebviewWindowBuilder;
#[cfg(not(target_os = "macos"))]
use tauri::Emitter;
#[cfg(not(target_os = "macos"))]
use tauri::WebviewUrl;
use tauri::{AppHandle, Manager, State};

#[cfg(target_os = "windows")]
#[link(name = "User32")]
extern "system" {
    fn GetAsyncKeyState(v_key: i32) -> i16;
}

#[cfg(target_os = "windows")]
#[link(name = "winmm")]
extern "system" {
    fn mciSendStringW(
        command: *const u16,
        return_value: *mut u16,
        return_length: u32,
        callback: usize,
    ) -> u32;
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Reminder {
    id: String,
    title: String,
    message: String,
    interval_value: u32,
    interval_unit: String,
    #[serde(default = "default_reminder_animation")]
    animation: String,
    #[serde(default = "default_reminder_visible_for_seconds")]
    visible_for_seconds: u32,
    #[serde(default)]
    sound_enabled: bool,
    #[serde(default = "default_sound_cue_id")]
    sound_cue_id: String,
    enabled: bool,
    next_run_at: u64,
}

#[derive(Clone)]
struct ReminderRepository {
    reminders: Arc<Mutex<Vec<Reminder>>>,
    path: Arc<PathBuf>,
}

const DEFAULT_PET_SIZE: u32 = 152;
const MIN_PET_SIZE: u32 = 96;
const MAX_PET_SIZE: u32 = 224;
const BUILTIN_CHARACTER_ID: &str = "builtin-baalert";
const LEGACY_BUILTIN_CHARACTER_ID: &str = "builtin-flyingsheep";
const MAX_CHARACTER_FILES: usize = 500;
const MAX_CHARACTER_BYTES: usize = 40 * 1024 * 1024;
const MAX_SOUND_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_SOUND_CUE_ID: &str = "builtin-gentle-chime";

fn default_bubble_style() -> String {
    "lime".to_string()
}

fn default_character_id() -> String {
    BUILTIN_CHARACTER_ID.to_string()
}

fn default_reminder_animation() -> String {
    "idle".to_string()
}

fn default_sound_cue_id() -> String {
    DEFAULT_SOUND_CUE_ID.to_string()
}

fn default_sound_enabled() -> bool {
    true
}

fn default_sound_volume() -> u8 {
    70
}

fn normalize_sound_cue_id(id: &str) -> String {
    let id = id.trim();
    if matches!(
        id,
        "builtin-gentle-chime" | "builtin-bright-pop" | "builtin-soft-bell"
    ) || (id.starts_with("custom-") && is_safe_character_id(id))
    {
        id.to_string()
    } else {
        default_sound_cue_id()
    }
}

fn default_reminder_visible_for_seconds() -> u32 {
    10
}

fn normalize_reminder_visible_for_seconds(seconds: u32) -> u32 {
    if seconds == 0 {
        0
    } else {
        seconds.clamp(2, 3600)
    }
}

fn normalize_reminder_animation(animation: &str) -> String {
    let normalized = animation.trim().replace('\\', "/");
    match normalized.as_str() {
        "idle" | "run-left" | "run-right" | "hover" => normalized,
        _ => {
            let key = normalize_animation_key(normalized.trim_start_matches("custom/"));
            if key.is_empty() {
                default_reminder_animation()
            } else {
                format!("custom/{key}")
            }
        }
    }
}

fn normalize_bubble_style(style: &str) -> String {
    match style {
        "lime" | "pink" | "yellow" | "cyan" => style.to_string(),
        _ => default_bubble_style(),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PetSettings {
    pet_size: u32,
    #[serde(default = "default_bubble_style")]
    bubble_style: String,
    #[serde(default = "default_character_id")]
    active_character_id: String,
    #[serde(default)]
    dark_mode: bool,
    #[serde(default = "default_sound_enabled")]
    sound_enabled: bool,
    #[serde(default = "default_sound_volume")]
    sound_volume: u8,
}

fn default_pet_settings() -> PetSettings {
    PetSettings {
        pet_size: DEFAULT_PET_SIZE,
        bubble_style: default_bubble_style(),
        active_character_id: default_character_id(),
        dark_mode: false,
        sound_enabled: default_sound_enabled(),
        sound_volume: default_sound_volume(),
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CharacterImportFile {
    animation: String,
    file_name: String,
    bytes: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CharacterAnimationImportFile {
    file_name: String,
    bytes: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CharacterManifest {
    id: String,
    name: String,
    imported_at: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CustomSoundCueManifest {
    id: String,
    name: String,
    file_name: String,
    imported_at: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SoundCueSummary {
    id: String,
    name: String,
    is_builtin: bool,
    format: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CharacterAnimationSummary {
    id: String,
    name: String,
    kind: String,
    frame_count: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CharacterPack {
    id: String,
    name: String,
    is_builtin: bool,
    is_ready: bool,
    preview_data_url: String,
    animations: Vec<CharacterAnimationSummary>,
    total_frames: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PetOverlayFrameSet {
    idle: Vec<String>,
    run_left: Vec<String>,
    run_right: Vec<String>,
    hover: Vec<String>,
    reminder: Vec<String>,
}

#[cfg(not(target_os = "macos"))]
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PetOverlayPayload {
    revision: u64,
    title: String,
    message: String,
    show_after_seconds: u32,
    visible_for_seconds: u32,
    show_bubble: bool,
    pet_size: u32,
    bubble_style: String,
    reminder_animation: String,
    sound_data_url: Option<String>,
    sound_volume: u8,
    dark_mode: bool,
}

#[derive(Clone)]
struct PetSettingsRepository {
    settings: Arc<Mutex<PetSettings>>,
    path: Arc<PathBuf>,
}

fn current_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn reminder_interval_millis(value: u32, unit: &str) -> Result<u64, String> {
    match unit {
        "minutes" if (1..=60).contains(&value) => Ok(value as u64 * 60_000),
        "hours" if (1..=24).contains(&value) => Ok(value as u64 * 3_600_000),
        "minutes" => Err("Minute intervals must be between 1 and 60".to_string()),
        "hours" => Err("Hour intervals must be between 1 and 24".to_string()),
        _ => Err("Interval unit must be minutes or hours".to_string()),
    }
}

fn persist_reminders(
    repository: &ReminderRepository,
    reminders: &[Reminder],
) -> Result<(), String> {
    if let Some(parent) = repository.path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let json = serde_json::to_string_pretty(reminders).map_err(|error| error.to_string())?;
    fs::write(repository.path.as_ref(), json).map_err(|error| error.to_string())
}

fn create_reminder_repository(app: &AppHandle) -> Result<ReminderRepository, String> {
    let path = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?
        .join("reminders.json");
    let reminders = fs::read_to_string(&path)
        .ok()
        .and_then(|contents| serde_json::from_str::<Vec<Reminder>>(&contents).ok())
        .unwrap_or_default();

    Ok(ReminderRepository {
        reminders: Arc::new(Mutex::new(reminders)),
        path: Arc::new(path),
    })
}

fn create_pet_settings_repository(app: &AppHandle) -> Result<PetSettingsRepository, String> {
    let path = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?
        .join("settings.json");
    let mut settings = fs::read_to_string(&path)
        .ok()
        .and_then(|contents| serde_json::from_str::<PetSettings>(&contents).ok())
        .unwrap_or_else(default_pet_settings);
    settings.pet_size = settings.pet_size.clamp(MIN_PET_SIZE, MAX_PET_SIZE);
    settings.bubble_style = normalize_bubble_style(&settings.bubble_style);
    settings.sound_volume = settings.sound_volume.min(100);
    if settings.active_character_id.trim().is_empty()
        || settings.active_character_id == LEGACY_BUILTIN_CHARACTER_ID
    {
        settings.active_character_id = default_character_id();
    }

    Ok(PetSettingsRepository {
        settings: Arc::new(Mutex::new(settings)),
        path: Arc::new(path),
    })
}

fn persist_pet_settings(repository: &PetSettingsRepository) -> Result<(), String> {
    if let Some(parent) = repository.path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let settings = repository
        .settings
        .lock()
        .map_err(|error| error.to_string())?;
    let json = serde_json::to_string_pretty(&*settings).map_err(|error| error.to_string())?;
    fs::write(repository.path.as_ref(), json).map_err(|error| error.to_string())
}

fn characters_directory(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?
        .join("characters"))
}

fn sounds_directory(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?
        .join("sounds"))
}

fn sound_manifest_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(sounds_directory(app)?.join("library.json"))
}

fn builtin_sound_cues() -> Vec<SoundCueSummary> {
    [
        ("builtin-gentle-chime", "Gentle Chime"),
        ("builtin-bright-pop", "Bright Pop"),
        ("builtin-soft-bell", "Soft Bell"),
    ]
    .into_iter()
    .map(|(id, name)| SoundCueSummary {
        id: id.to_string(),
        name: name.to_string(),
        is_builtin: true,
        format: "WAV".to_string(),
    })
    .collect()
}

fn read_custom_sound_manifest(app: &AppHandle) -> Vec<CustomSoundCueManifest> {
    sound_manifest_path(app)
        .ok()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .unwrap_or_default()
}

fn persist_custom_sound_manifest(
    app: &AppHandle,
    sounds: &[CustomSoundCueManifest],
) -> Result<(), String> {
    let directory = sounds_directory(app)?;
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let json = serde_json::to_string_pretty(sounds).map_err(|error| error.to_string())?;
    fs::write(directory.join("library.json"), json).map_err(|error| error.to_string())
}

fn sound_file_format(file_name: &str) -> Option<(&'static str, &'static str)> {
    match Path::new(file_name)
        .extension()
        .and_then(|extension| extension.to_str())?
        .to_ascii_lowercase()
        .as_str()
    {
        "mp3" => Some(("mp3", "audio/mpeg")),
        "wav" => Some(("wav", "audio/wav")),
        _ => None,
    }
}

fn validate_sound_file(
    file_name: &str,
    bytes: &[u8],
) -> Result<(&'static str, &'static str), String> {
    if bytes.is_empty() {
        return Err("The selected sound file is empty".to_string());
    }
    if bytes.len() > MAX_SOUND_BYTES {
        return Err("Sound files must stay under 8 MB".to_string());
    }

    let (extension, mime) = sound_file_format(file_name)
        .ok_or_else(|| "Choose an MP3 or WAV sound file".to_string())?;
    let valid_header = match extension {
        "mp3" => {
            bytes.starts_with(b"ID3")
                || bytes
                    .get(..2)
                    .is_some_and(|header| header[0] == 0xff && header[1] & 0xe0 == 0xe0)
        }
        "wav" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE"),
        _ => false,
    };

    if valid_header {
        Ok((extension, mime))
    } else {
        Err(format!(
            "{} does not appear to be a valid {extension} file",
            file_name
        ))
    }
}

fn wav_bytes(duration_seconds: f32, tones: &[(f32, f32, f32, f32)]) -> Vec<u8> {
    const SAMPLE_RATE: u32 = 44_100;
    const CHANNELS: u16 = 1;
    const BITS_PER_SAMPLE: u16 = 16;

    let sample_count = (duration_seconds * SAMPLE_RATE as f32).ceil() as usize;
    let mut samples = Vec::with_capacity(sample_count);
    for index in 0..sample_count {
        let time = index as f32 / SAMPLE_RATE as f32;
        let mut value = 0.0f32;
        for &(start, duration, frequency, amplitude) in tones {
            if !(start..start + duration).contains(&time) {
                continue;
            }
            let local_time = time - start;
            let progress = local_time / duration;
            let attack = (local_time / 0.018).min(1.0);
            let envelope = attack * (1.0 - progress).powf(2.2);
            let phase = std::f32::consts::TAU * frequency * local_time;
            value += amplitude * envelope * (phase.sin() + 0.22 * (phase * 2.0).sin());
        }
        samples.push((value.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
    }

    let data_size = (samples.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + data_size as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_size).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&CHANNELS.to_le_bytes());
    bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    bytes.extend_from_slice(&(SAMPLE_RATE * CHANNELS as u32 * 2).to_le_bytes());
    bytes.extend_from_slice(&(CHANNELS * 2).to_le_bytes());
    bytes.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_size.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

fn builtin_sound_bytes(id: &str) -> Option<Vec<u8>> {
    match id {
        "builtin-gentle-chime" => Some(wav_bytes(
            0.95,
            &[
                (0.00, 0.46, 523.25, 0.34),
                (0.18, 0.46, 659.25, 0.30),
                (0.36, 0.56, 783.99, 0.27),
            ],
        )),
        "builtin-bright-pop" => Some(wav_bytes(
            0.42,
            &[(0.00, 0.24, 880.00, 0.40), (0.12, 0.28, 1_318.51, 0.34)],
        )),
        "builtin-soft-bell" => Some(wav_bytes(
            1.20,
            &[(0.00, 1.15, 659.25, 0.31), (0.00, 0.85, 987.77, 0.16)],
        )),
        _ => None,
    }
}

fn custom_sound_path(app: &AppHandle, sound: &CustomSoundCueManifest) -> Result<PathBuf, String> {
    let file_name = Path::new(&sound.file_name);
    if file_name.file_name().and_then(|name| name.to_str()) != Some(sound.file_name.as_str()) {
        return Err("Invalid stored sound file name".to_string());
    }
    Ok(sounds_directory(app)?.join(file_name))
}

fn sound_cue_bytes(app: &AppHandle, id: &str) -> Result<(Vec<u8>, &'static str), String> {
    if let Some(bytes) = builtin_sound_bytes(id) {
        return Ok((bytes, "audio/wav"));
    }

    let sound = read_custom_sound_manifest(app)
        .into_iter()
        .find(|sound| sound.id == id)
        .ok_or_else(|| "Sound cue not found".to_string())?;
    let path = custom_sound_path(app, &sound)?;
    let (_, mime) = sound_file_format(&sound.file_name)
        .ok_or_else(|| "Stored sound format is not supported".to_string())?;
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    validate_sound_file(&sound.file_name, &bytes)?;
    Ok((bytes, mime))
}

#[cfg(target_os = "windows")]
fn sound_cue_path(app: &AppHandle, id: &str) -> Result<PathBuf, String> {
    if let Some(bytes) = builtin_sound_bytes(id) {
        let directory = sounds_directory(app)?.join("built-in");
        fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        let path = directory.join(format!("{id}.wav"));
        if !path.is_file() {
            fs::write(&path, bytes).map_err(|error| error.to_string())?;
        }
        return Ok(path);
    }

    let sound = read_custom_sound_manifest(app)
        .into_iter()
        .find(|sound| sound.id == id)
        .ok_or_else(|| "Sound cue not found".to_string())?;
    custom_sound_path(app, &sound)
}

#[tauri::command]
fn list_sound_cues(app: AppHandle) -> Result<Vec<SoundCueSummary>, String> {
    let mut sounds = builtin_sound_cues();
    for sound in read_custom_sound_manifest(&app) {
        let path = custom_sound_path(&app, &sound)?;
        let Some((extension, _)) = sound_file_format(&sound.file_name) else {
            continue;
        };
        if path.is_file() {
            sounds.push(SoundCueSummary {
                id: sound.id,
                name: sound.name,
                is_builtin: false,
                format: extension.to_ascii_uppercase(),
            });
        }
    }
    Ok(sounds)
}

#[tauri::command]
fn import_sound_cue(
    app: AppHandle,
    name: String,
    file_name: String,
    bytes: Vec<u8>,
) -> Result<SoundCueSummary, String> {
    let name: String = name.trim().chars().take(48).collect();
    if name.is_empty() {
        return Err("Sound cue name is required".to_string());
    }
    let (extension, _) = validate_sound_file(&file_name, &bytes)?;
    let id = format!(
        "custom-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let stored_file_name = format!("{id}.{extension}");
    let directory = sounds_directory(&app)?;
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    fs::write(directory.join(&stored_file_name), bytes).map_err(|error| error.to_string())?;

    let mut manifest = read_custom_sound_manifest(&app);
    manifest.push(CustomSoundCueManifest {
        id: id.clone(),
        name: name.clone(),
        file_name: stored_file_name,
        imported_at: current_time_millis(),
    });
    persist_custom_sound_manifest(&app, &manifest)?;

    Ok(SoundCueSummary {
        id,
        name,
        is_builtin: false,
        format: extension.to_ascii_uppercase(),
    })
}

#[tauri::command]
fn delete_sound_cue(
    app: AppHandle,
    reminders: State<'_, ReminderRepository>,
    id: String,
) -> Result<(), String> {
    if !id.starts_with("custom-") {
        return Err("Built-in sound cues cannot be deleted".to_string());
    }
    let mut manifest = read_custom_sound_manifest(&app);
    let index = manifest
        .iter()
        .position(|sound| sound.id == id)
        .ok_or_else(|| "Sound cue not found".to_string())?;
    let sound = manifest.remove(index);
    let path = custom_sound_path(&app, &sound)?;
    if path.is_file() {
        fs::remove_file(path).map_err(|error| error.to_string())?;
    }
    persist_custom_sound_manifest(&app, &manifest)?;

    let mut saved_reminders = reminders
        .reminders
        .lock()
        .map_err(|error| error.to_string())?;
    let mut changed = false;
    for reminder in saved_reminders
        .iter_mut()
        .filter(|reminder| reminder.sound_cue_id == id)
    {
        reminder.sound_enabled = false;
        reminder.sound_cue_id = default_sound_cue_id();
        changed = true;
    }
    if changed {
        persist_reminders(&reminders, &saved_reminders)?;
    }
    Ok(())
}

#[tauri::command]
fn get_sound_cue_data_url(app: AppHandle, id: String) -> Result<String, String> {
    let (bytes, mime) = sound_cue_bytes(&app, &id)?;
    Ok(format!(
        "data:{mime};base64,{}",
        BASE64_STANDARD.encode(bytes)
    ))
}

fn resolve_active_character_root(app: &AppHandle) -> Result<PathBuf, String> {
    let active_character_id = app
        .state::<PetSettingsRepository>()
        .settings
        .lock()
        .map(|settings| settings.active_character_id.clone())
        .unwrap_or_else(|_| default_character_id());
    let imported_root = characters_directory(app)?.join(&active_character_id);

    if active_character_id != BUILTIN_CHARACTER_ID
        && imported_root.is_dir()
        && character_is_ready(&imported_root)
    {
        Ok(imported_root)
    } else {
        resolve_builtin_character_root(app)
    }
}

#[tauri::command]
fn get_animation_preview_frames(
    app: AppHandle,
    animation_id: String,
    character_id: Option<String>,
) -> Result<Vec<String>, String> {
    let root = match character_id {
        Some(character_id) => {
            if !is_safe_character_id(&character_id) {
                return Err("Invalid character identifier".to_string());
            }
            if character_id == BUILTIN_CHARACTER_ID {
                resolve_builtin_character_root(&app)?
            } else {
                let root = characters_directory(&app)?.join(character_id);
                if !root.is_dir() {
                    return Err("Character not found".to_string());
                }
                root
            }
        }
        None => resolve_active_character_root(&app)?,
    };
    let animation_id = normalize_reminder_animation(&animation_id);
    let directory = match animation_id.as_str() {
        "idle" => root.join("idle"),
        "run-left" => root.join("run-left"),
        "run-right" => root.join("run-right"),
        custom => root.join(custom),
    };
    let frames = collect_png_frames(&directory)?;

    frames
        .into_iter()
        .map(|frame| {
            let bytes = fs::read(frame).map_err(|error| error.to_string())?;
            Ok(format!(
                "data:image/png;base64,{}",
                BASE64_STANDARD.encode(bytes)
            ))
        })
        .collect()
}

fn animation_data_urls(root: &Path, animation_id: &str) -> Result<Vec<String>, String> {
    let frames = collect_png_frames(&root.join(animation_id)).unwrap_or_default();
    frames
        .into_iter()
        .map(|frame| {
            let bytes = fs::read(frame).map_err(|error| error.to_string())?;
            Ok(format!(
                "data:image/png;base64,{}",
                BASE64_STANDARD.encode(bytes)
            ))
        })
        .collect()
}

#[tauri::command]
fn get_pet_overlay_frames(
    app: AppHandle,
    reminder_animation: String,
) -> Result<PetOverlayFrameSet, String> {
    let root = resolve_active_character_root(&app)?;
    let reminder_animation = normalize_reminder_animation(&reminder_animation);
    let reminder = match reminder_animation.as_str() {
        "idle" | "run-left" | "run-right" | "hover" => Vec::new(),
        custom => animation_data_urls(&root, custom)?,
    };
    let frames = PetOverlayFrameSet {
        idle: animation_data_urls(&root, "idle")?,
        run_left: animation_data_urls(&root, "run-left")?,
        run_right: animation_data_urls(&root, "run-right")?,
        hover: animation_data_urls(&root, "hover")?,
        reminder,
    };

    if frames.idle.is_empty()
        && frames.run_left.is_empty()
        && frames.run_right.is_empty()
        && frames.hover.is_empty()
        && frames.reminder.is_empty()
    {
        Err("The active character does not contain any animation frames".to_string())
    } else {
        Ok(frames)
    }
}

fn resolve_builtin_character_root(app: &AppHandle) -> Result<PathBuf, String> {
    let mut candidates = Vec::new();

    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.push(resource_dir.join("character"));
        candidates.push(resource_dir.join("resources").join("character"));
    }

    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join("character"),
    );
    candidates.push(PathBuf::from("src-tauri/resources/character"));
    candidates.push(PathBuf::from("resources/character"));

    candidates
        .into_iter()
        .find(|path| path.is_dir())
        .ok_or_else(|| "Built-in character frames were not found".to_string())
}

fn normalize_animation_key(value: &str) -> String {
    let leaf = value
        .replace('\\', "/")
        .split('/')
        .filter(|part| !part.trim().is_empty())
        .next_back()
        .unwrap_or(value)
        .trim()
        .to_lowercase();
    let mut key = String::new();
    let mut previous_dash = false;

    for character in leaf.chars() {
        if character.is_alphanumeric() {
            key.push(character);
            previous_dash = false;
        } else if !previous_dash && !key.is_empty() {
            key.push('-');
            previous_dash = true;
        }
    }

    key.trim_matches('-').to_string()
}

fn classify_animation(value: &str) -> Result<(String, String), String> {
    let normalized = value.replace('\\', "/");
    let explicit_custom = normalized
        .split('/')
        .next()
        .is_some_and(|part| matches!(part.trim().to_lowercase().as_str(), "custom" | "animations"));
    let key = normalize_animation_key(value);
    if explicit_custom {
        return if key.is_empty() || matches!(key.as_str(), "custom" | "animations") {
            Err(format!(
                "Animation folder '{value}' needs a specific animation name"
            ))
        } else {
            Ok((format!("custom/{key}"), title_from_slug(&key)))
        };
    }
    match key.as_str() {
        "idle" | "idling" | "front-idle" => Ok(("idle".to_string(), "Idle".to_string())),
        "run-left" | "running-left" | "left" => {
            Ok(("run-left".to_string(), "Run left".to_string()))
        }
        "run-right" | "running-right" | "right" => {
            Ok(("run-right".to_string(), "Run right".to_string()))
        }
        "hover" | "hovering" | "mouse-hover" => Ok(("hover".to_string(), "Hover".to_string())),
        "" | "animations" | "custom" => Err(format!(
            "Animation folder '{value}' needs a specific animation name"
        )),
        _ => Ok((format!("custom/{key}"), title_from_slug(&key))),
    }
}

fn title_from_slug(slug: &str) -> String {
    slug.split('-')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            characters
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + characters.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn character_id_slug(name: &str) -> String {
    let slug = normalize_animation_key(name);
    if slug.is_empty() {
        "character".to_string()
    } else {
        slug
    }
}

fn is_safe_character_id(id: &str) -> bool {
    !id.is_empty() && id != "." && id != ".." && !id.contains('/') && !id.contains('\\')
}

fn animation_frame_count(directory: &Path) -> usize {
    collect_png_frames(directory)
        .map(|frames| frames.len())
        .unwrap_or(0)
}

fn character_is_ready(root: &Path) -> bool {
    character_png_usage(root, None)
        .map(|(frame_count, _)| frame_count > 0)
        .unwrap_or(false)
}

fn character_png_usage(root: &Path, excluded: Option<&Path>) -> Result<(usize, usize), String> {
    if excluded.is_some_and(|path| root == path) || !root.exists() {
        return Ok((0, 0));
    }

    let mut file_count = 0usize;
    let mut total_bytes = 0usize;
    for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let (nested_count, nested_bytes) = character_png_usage(&path, excluded)?;
            file_count = file_count
                .checked_add(nested_count)
                .ok_or_else(|| "Character contains too many files".to_string())?;
            total_bytes = total_bytes
                .checked_add(nested_bytes)
                .ok_or_else(|| "Character files are too large".to_string())?;
        } else if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
        {
            file_count = file_count
                .checked_add(1)
                .ok_or_else(|| "Character contains too many files".to_string())?;
            total_bytes = total_bytes
                .checked_add(entry.metadata().map_err(|error| error.to_string())?.len() as usize)
                .ok_or_else(|| "Character files are too large".to_string())?;
        }
    }

    Ok((file_count, total_bytes))
}

fn character_pack_from_root(
    root: &Path,
    id: String,
    name: String,
    is_builtin: bool,
) -> Result<CharacterPack, String> {
    let idle_frames = collect_png_frames(&root.join("idle")).unwrap_or_default();
    let run_left_frames = collect_png_frames(&root.join("run-left")).unwrap_or_default();
    let run_right_frames = collect_png_frames(&root.join("run-right")).unwrap_or_default();
    let hover_frames = collect_png_frames(&root.join("hover")).unwrap_or_default();
    let mut animations = Vec::new();
    if !idle_frames.is_empty() {
        animations.push(CharacterAnimationSummary {
            id: "idle".to_string(),
            name: "Idle".to_string(),
            kind: "idle".to_string(),
            frame_count: idle_frames.len(),
        });
    }
    if !run_left_frames.is_empty() {
        animations.push(CharacterAnimationSummary {
            id: "run-left".to_string(),
            name: "Run left".to_string(),
            kind: "runLeft".to_string(),
            frame_count: run_left_frames.len(),
        });
    }
    if !run_right_frames.is_empty() {
        animations.push(CharacterAnimationSummary {
            id: "run-right".to_string(),
            name: "Run right".to_string(),
            kind: "runRight".to_string(),
            frame_count: run_right_frames.len(),
        });
    }
    if !hover_frames.is_empty() {
        animations.push(CharacterAnimationSummary {
            id: "hover".to_string(),
            name: "Hover".to_string(),
            kind: "hover".to_string(),
            frame_count: hover_frames.len(),
        });
    }

    let custom_root = root.join("custom");
    let mut first_custom_preview = None;
    if custom_root.is_dir() {
        let mut custom_directories = fs::read_dir(&custom_root)
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect::<Vec<_>>();
        custom_directories.sort();

        for directory in custom_directories {
            let frame_count = animation_frame_count(&directory);
            if frame_count == 0 {
                continue;
            }
            let slug = directory
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("custom");
            if first_custom_preview.is_none() {
                first_custom_preview = collect_png_frames(&directory)
                    .ok()
                    .and_then(|frames| frames.into_iter().next());
            }
            animations.push(CharacterAnimationSummary {
                id: format!("custom/{slug}"),
                name: title_from_slug(slug),
                kind: "custom".to_string(),
                frame_count,
            });
        }
    }

    let total_frames = animations
        .iter()
        .map(|animation| animation.frame_count)
        .sum();
    let is_ready = total_frames > 0;
    if is_builtin && !is_ready {
        return Err(format!(
            "Character '{name}' does not contain any animations"
        ));
    }
    let preview_path = idle_frames
        .first()
        .or_else(|| run_left_frames.first())
        .or_else(|| run_right_frames.first())
        .or_else(|| hover_frames.first())
        .or(first_custom_preview.as_ref());
    let preview_data_url = preview_path
        .map(fs::read)
        .transpose()
        .map_err(|error| error.to_string())?
        .map(|bytes| format!("data:image/png;base64,{}", BASE64_STANDARD.encode(bytes)))
        .unwrap_or_default();
    Ok(CharacterPack {
        id,
        name,
        is_builtin,
        is_ready,
        preview_data_url,
        animations,
        total_frames,
    })
}

fn load_character_manifest(root: &Path) -> Result<CharacterManifest, String> {
    let contents =
        fs::read_to_string(root.join("character.json")).map_err(|error| error.to_string())?;
    serde_json::from_str(&contents).map_err(|error| error.to_string())
}

#[tauri::command]
fn list_characters(app: AppHandle) -> Result<Vec<CharacterPack>, String> {
    let builtin_root = resolve_builtin_character_root(&app)?;
    let mut characters = vec![character_pack_from_root(
        &builtin_root,
        BUILTIN_CHARACTER_ID.to_string(),
        "Baalert".to_string(),
        true,
    )?];
    let characters_root = characters_directory(&app)?;
    fs::create_dir_all(&characters_root).map_err(|error| error.to_string())?;

    let mut imported = fs::read_dir(&characters_root)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .filter_map(|root| {
            let manifest = load_character_manifest(&root).ok()?;
            character_pack_from_root(&root, manifest.id, manifest.name, false).ok()
        })
        .collect::<Vec<_>>();
    imported.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
    characters.extend(imported);
    Ok(characters)
}

#[tauri::command]
fn create_character(app: AppHandle, name: String) -> Result<CharacterPack, String> {
    let name: String = name.trim().chars().take(48).collect();
    if name.is_empty() {
        return Err("Character name is required".to_string());
    }

    let id = format!("{}-{}", character_id_slug(&name), current_time_millis());
    let root = characters_directory(&app)?.join(&id);
    fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let manifest = CharacterManifest {
        id: id.clone(),
        name: name.clone(),
        imported_at: current_time_millis(),
    };
    let json = serde_json::to_string_pretty(&manifest).map_err(|error| error.to_string())?;
    if let Err(error) = fs::write(root.join("character.json"), json) {
        let _ = fs::remove_dir_all(&root);
        return Err(error.to_string());
    }

    character_pack_from_root(&root, id, name, false)
}

#[tauri::command]
fn add_character_animation(
    app: AppHandle,
    character_id: String,
    animation: String,
    files: Vec<CharacterAnimationImportFile>,
) -> Result<CharacterPack, String> {
    if !is_safe_character_id(&character_id) || character_id == BUILTIN_CHARACTER_ID {
        return Err("Animations can only be added to a local character".to_string());
    }
    if files.is_empty() {
        return Err("The selected animation folder does not contain PNG frames".to_string());
    }

    let root = characters_directory(&app)?.join(&character_id);
    if !root.is_dir() {
        return Err("Character not found".to_string());
    }
    let manifest = load_character_manifest(&root)?;
    if manifest.id != character_id {
        return Err("Character data is invalid".to_string());
    }

    let (animation_directory, _) = classify_animation(&animation)?;
    let target = root.join(&animation_directory);
    let upload_bytes = files.iter().try_fold(0usize, |total, file| {
        if file.bytes.len() < 8 || file.bytes[..8] != [137, 80, 78, 71, 13, 10, 26, 10] {
            return Err(format!("{} is not a valid PNG file", file.file_name));
        }
        total
            .checked_add(file.bytes.len())
            .ok_or_else(|| "Animation files are too large".to_string())
    })?;
    let (existing_count, existing_bytes) = character_png_usage(&root, Some(&target))?;
    if existing_count.saturating_add(files.len()) > MAX_CHARACTER_FILES {
        return Err(format!(
            "A character can contain up to {MAX_CHARACTER_FILES} PNG frames"
        ));
    }
    if existing_bytes.saturating_add(upload_bytes) > MAX_CHARACTER_BYTES {
        return Err("Character files must stay under 40 MB".to_string());
    }

    let parent = target
        .parent()
        .ok_or_else(|| "Invalid animation folder".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let leaf = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("animation");
    let operation_id = current_time_millis();
    let staging = parent.join(format!(".{leaf}-{operation_id}.importing"));
    let backup = parent.join(format!(".{leaf}-{operation_id}.backup"));
    fs::create_dir_all(&staging).map_err(|error| error.to_string())?;

    let write_result = (|| -> Result<(), String> {
        for (index, file) in files.iter().enumerate() {
            fs::write(staging.join(format!("{index:04}.png")), &file.bytes)
                .map_err(|error| error.to_string())?;
        }
        if target.exists() {
            fs::rename(&target, &backup).map_err(|error| error.to_string())?;
        }
        if let Err(error) = fs::rename(&staging, &target) {
            if backup.exists() {
                let _ = fs::rename(&backup, &target);
            }
            return Err(error.to_string());
        }
        if backup.exists() {
            fs::remove_dir_all(&backup).map_err(|error| error.to_string())?;
        }
        Ok(())
    })();

    if let Err(error) = write_result {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }

    character_pack_from_root(&root, manifest.id, manifest.name, false)
}

#[tauri::command]
fn delete_character_animation(
    app: AppHandle,
    repository: State<'_, PetSettingsRepository>,
    character_id: String,
    animation_id: String,
) -> Result<PetSettings, String> {
    if !is_safe_character_id(&character_id) || character_id == BUILTIN_CHARACTER_ID {
        return Err("Animations can only be deleted from a local character".to_string());
    }

    let root = characters_directory(&app)?.join(&character_id);
    if !root.is_dir() {
        return Err("Character not found".to_string());
    }
    let manifest = load_character_manifest(&root)?;
    if manifest.id != character_id {
        return Err("Character data is invalid".to_string());
    }

    let (animation_directory, _) = classify_animation(&animation_id)?;
    let target = root.join(animation_directory);
    if !target.is_dir() {
        return Err("Animation not found".to_string());
    }
    fs::remove_dir_all(&target).map_err(|error| error.to_string())?;
    if let Some(parent) = target.parent() {
        if parent.file_name().and_then(|name| name.to_str()) == Some("custom")
            && fs::read_dir(parent)
                .map(|mut entries| entries.next().is_none())
                .unwrap_or(false)
        {
            let _ = fs::remove_dir(parent);
        }
    }

    let updated = {
        let mut settings = repository
            .settings
            .lock()
            .map_err(|error| error.to_string())?;
        if settings.active_character_id == character_id && !character_is_ready(&root) {
            settings.active_character_id = default_character_id();
        }
        settings.clone()
    };
    persist_pet_settings(&repository)?;
    Ok(updated)
}

#[tauri::command]
fn import_character(
    app: AppHandle,
    repository: State<'_, PetSettingsRepository>,
    name: String,
    files: Vec<CharacterImportFile>,
) -> Result<CharacterPack, String> {
    let name: String = name.trim().chars().take(48).collect();
    if name.is_empty() {
        return Err("Character name is required".to_string());
    }
    if files.is_empty() || files.len() > MAX_CHARACTER_FILES {
        return Err(format!(
            "A character must contain between 1 and {MAX_CHARACTER_FILES} PNG frames"
        ));
    }

    let total_bytes = files.iter().try_fold(0usize, |total, file| {
        total
            .checked_add(file.bytes.len())
            .ok_or_else(|| "Character files are too large".to_string())
    })?;
    if total_bytes > MAX_CHARACTER_BYTES {
        return Err("Character files must stay under 40 MB".to_string());
    }

    let id = format!("{}-{}", character_id_slug(&name), current_time_millis());
    let characters_root = characters_directory(&app)?;
    let import_root = characters_root.join(format!("{id}.importing"));
    let final_root = characters_root.join(&id);
    fs::create_dir_all(&import_root).map_err(|error| error.to_string())?;

    let import_result = (|| -> Result<(), String> {
        for (index, file) in files.iter().enumerate() {
            if file.bytes.len() < 8 || file.bytes[..8] != [137, 80, 78, 71, 13, 10, 26, 10] {
                return Err(format!("{} is not a valid PNG file", file.file_name));
            }
            let (animation_directory, _) = classify_animation(&file.animation)?;
            let target_directory = import_root.join(animation_directory);
            fs::create_dir_all(&target_directory).map_err(|error| error.to_string())?;
            fs::write(
                target_directory.join(format!("{index:04}.png")),
                &file.bytes,
            )
            .map_err(|error| error.to_string())?;
        }

        let manifest = CharacterManifest {
            id: id.clone(),
            name: name.clone(),
            imported_at: current_time_millis(),
        };
        let json = serde_json::to_string_pretty(&manifest).map_err(|error| error.to_string())?;
        fs::write(import_root.join("character.json"), json).map_err(|error| error.to_string())?;
        fs::rename(&import_root, &final_root).map_err(|error| error.to_string())?;
        Ok(())
    })();

    if let Err(error) = import_result {
        let _ = fs::remove_dir_all(&import_root);
        return Err(error);
    }

    {
        let mut settings = repository
            .settings
            .lock()
            .map_err(|error| error.to_string())?;
        settings.active_character_id = id.clone();
    }
    persist_pet_settings(&repository)?;
    character_pack_from_root(&final_root, id, name, false)
}

#[tauri::command]
fn set_active_character(
    app: AppHandle,
    repository: State<'_, PetSettingsRepository>,
    character_id: String,
) -> Result<PetSettings, String> {
    if !is_safe_character_id(&character_id) {
        return Err("Invalid character identifier".to_string());
    }
    let character_root = if character_id == BUILTIN_CHARACTER_ID {
        resolve_builtin_character_root(&app)?
    } else {
        characters_directory(&app)?.join(&character_id)
    };
    if !character_root.is_dir() {
        return Err("Character not found".to_string());
    }
    if !character_is_ready(&character_root) {
        return Err("Add at least one animation before using this character".to_string());
    }

    let updated = {
        let mut settings = repository
            .settings
            .lock()
            .map_err(|error| error.to_string())?;
        settings.active_character_id = character_id;
        settings.clone()
    };
    persist_pet_settings(&repository)?;
    Ok(updated)
}

#[tauri::command]
fn delete_character(
    app: AppHandle,
    repository: State<'_, PetSettingsRepository>,
    character_id: String,
) -> Result<PetSettings, String> {
    if !is_safe_character_id(&character_id) {
        return Err("Invalid character identifier".to_string());
    }
    if character_id == BUILTIN_CHARACTER_ID {
        return Err("The built-in character cannot be deleted".to_string());
    }
    let character_root = characters_directory(&app)?.join(&character_id);
    if !character_root.is_dir() {
        return Err("Character not found".to_string());
    }
    fs::remove_dir_all(character_root).map_err(|error| error.to_string())?;

    let updated = {
        let mut settings = repository
            .settings
            .lock()
            .map_err(|error| error.to_string())?;
        if settings.active_character_id == character_id {
            settings.active_character_id = default_character_id();
        }
        settings.clone()
    };
    persist_pet_settings(&repository)?;
    Ok(updated)
}

fn spawn_reminder_scheduler(
    app: AppHandle,
    repository: ReminderRepository,
    pet_settings: PetSettingsRepository,
) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(1));

        let now = current_time_millis();
        let mut due_reminders = Vec::new();
        let mut changed = false;

        if let Ok(mut reminders) = repository.reminders.lock() {
            for reminder in reminders.iter_mut().filter(|reminder| reminder.enabled) {
                if reminder.next_run_at <= now {
                    match reminder_interval_millis(reminder.interval_value, &reminder.interval_unit)
                    {
                        Ok(interval) => {
                            due_reminders.push((
                                reminder.title.clone(),
                                reminder.message.clone(),
                                reminder.animation.clone(),
                                reminder.visible_for_seconds,
                                reminder.sound_enabled,
                                reminder.sound_cue_id.clone(),
                            ));
                            reminder.next_run_at = now.saturating_add(interval);
                        }
                        Err(_) => reminder.enabled = false,
                    }
                    changed = true;
                }
            }

            if changed {
                let _ = persist_reminders(&repository, &reminders);
            }
        }

        for (
            title,
            message,
            animation,
            visible_for_seconds,
            reminder_sound_enabled,
            sound_cue_id,
        ) in due_reminders
        {
            let settings = pet_settings
                .settings
                .lock()
                .map(|settings| settings.clone())
                .unwrap_or_else(|_| default_pet_settings());
            let sound_cue_id =
                (settings.sound_enabled && reminder_sound_enabled).then_some(sound_cue_id);
            show_scheduled_reminder(
                app.clone(),
                title,
                message,
                animation,
                visible_for_seconds,
                sound_cue_id,
                settings.sound_volume,
                settings.pet_size,
                settings.bubble_style,
            );
            std::thread::sleep(Duration::from_secs(11));
        }
    });
}

fn show_scheduled_reminder(
    app: AppHandle,
    title: String,
    message: String,
    animation: String,
    visible_for_seconds: u32,
    sound_cue_id: Option<String>,
    sound_volume: u8,
    pet_size: u32,
    bubble_style: String,
) {
    tauri::async_runtime::spawn(async move {
        let _ = show_pet(
            app,
            Some(title),
            message,
            0,
            visible_for_seconds,
            true,
            pet_size,
            bubble_style,
            Some(animation),
            sound_cue_id,
            Some(sound_volume),
        )
        .await;
    });
}

#[tauri::command]
fn get_pet_settings(repository: State<'_, PetSettingsRepository>) -> Result<PetSettings, String> {
    repository
        .settings
        .lock()
        .map(|settings| settings.clone())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn set_pet_size(
    repository: State<'_, PetSettingsRepository>,
    pet_size: u32,
) -> Result<PetSettings, String> {
    let pet_size = pet_size.clamp(MIN_PET_SIZE, MAX_PET_SIZE);
    let updated = {
        let mut settings = repository
            .settings
            .lock()
            .map_err(|error| error.to_string())?;
        settings.pet_size = pet_size;
        settings.clone()
    };
    persist_pet_settings(&repository)?;
    Ok(updated)
}

#[tauri::command]
fn set_bubble_style(
    repository: State<'_, PetSettingsRepository>,
    bubble_style: String,
) -> Result<PetSettings, String> {
    let bubble_style = normalize_bubble_style(&bubble_style);
    let updated = {
        let mut settings = repository
            .settings
            .lock()
            .map_err(|error| error.to_string())?;
        settings.bubble_style = bubble_style;
        settings.clone()
    };
    persist_pet_settings(&repository)?;
    Ok(updated)
}

#[tauri::command]
fn set_dark_mode(
    repository: State<'_, PetSettingsRepository>,
    dark_mode: bool,
) -> Result<PetSettings, String> {
    let updated = {
        let mut settings = repository
            .settings
            .lock()
            .map_err(|error| error.to_string())?;
        settings.dark_mode = dark_mode;
        settings.clone()
    };
    persist_pet_settings(&repository)?;
    Ok(updated)
}

#[tauri::command]
fn set_sound_settings(
    repository: State<'_, PetSettingsRepository>,
    sound_enabled: bool,
    sound_volume: u8,
) -> Result<PetSettings, String> {
    let updated = {
        let mut settings = repository
            .settings
            .lock()
            .map_err(|error| error.to_string())?;
        settings.sound_enabled = sound_enabled;
        settings.sound_volume = sound_volume.min(100);
        settings.clone()
    };
    persist_pet_settings(&repository)?;
    Ok(updated)
}

#[tauri::command]
fn list_reminders(repository: State<'_, ReminderRepository>) -> Result<Vec<Reminder>, String> {
    repository
        .reminders
        .lock()
        .map(|reminders| reminders.clone())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn create_reminder(
    repository: State<'_, ReminderRepository>,
    title: String,
    message: String,
    interval_value: u32,
    interval_unit: String,
    animation: String,
    visible_for_seconds: u32,
    sound_enabled: bool,
    sound_cue_id: String,
) -> Result<Reminder, String> {
    let interval = reminder_interval_millis(interval_value, &interval_unit)?;
    let title: String = title.trim().chars().take(80).collect();
    let message: String = message.trim().chars().take(240).collect();
    if title.is_empty() {
        return Err("Reminder title is required".to_string());
    }
    if message.is_empty() {
        return Err("Reminder message is required".to_string());
    }

    let now = current_time_millis();
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string();
    let reminder = Reminder {
        id,
        title,
        message,
        interval_value,
        interval_unit,
        animation: normalize_reminder_animation(&animation),
        visible_for_seconds: normalize_reminder_visible_for_seconds(visible_for_seconds),
        sound_enabled,
        sound_cue_id: normalize_sound_cue_id(&sound_cue_id),
        enabled: true,
        next_run_at: now.saturating_add(interval),
    };

    let mut reminders = repository
        .reminders
        .lock()
        .map_err(|error| error.to_string())?;
    reminders.push(reminder.clone());
    persist_reminders(&repository, &reminders)?;
    Ok(reminder)
}

#[tauri::command]
fn update_reminder(
    repository: State<'_, ReminderRepository>,
    id: String,
    title: String,
    message: String,
    interval_value: u32,
    interval_unit: String,
    animation: String,
    visible_for_seconds: u32,
    sound_enabled: bool,
    sound_cue_id: String,
) -> Result<Reminder, String> {
    let interval = reminder_interval_millis(interval_value, &interval_unit)?;
    let title: String = title.trim().chars().take(80).collect();
    let message: String = message.trim().chars().take(240).collect();
    if title.is_empty() {
        return Err("Reminder title is required".to_string());
    }
    if message.is_empty() {
        return Err("Reminder message is required".to_string());
    }

    let mut reminders = repository
        .reminders
        .lock()
        .map_err(|error| error.to_string())?;
    let reminder = reminders
        .iter_mut()
        .find(|reminder| reminder.id == id)
        .ok_or_else(|| "Reminder not found".to_string())?;
    let schedule_changed =
        reminder.interval_value != interval_value || reminder.interval_unit != interval_unit;
    reminder.title = title;
    reminder.message = message;
    reminder.interval_value = interval_value;
    reminder.interval_unit = interval_unit;
    reminder.animation = normalize_reminder_animation(&animation);
    reminder.visible_for_seconds = normalize_reminder_visible_for_seconds(visible_for_seconds);
    reminder.sound_enabled = sound_enabled;
    reminder.sound_cue_id = normalize_sound_cue_id(&sound_cue_id);
    if reminder.enabled && schedule_changed {
        reminder.next_run_at = current_time_millis().saturating_add(interval);
    }
    let updated = reminder.clone();
    persist_reminders(&repository, &reminders)?;
    Ok(updated)
}

#[tauri::command]
fn set_reminder_enabled(
    repository: State<'_, ReminderRepository>,
    id: String,
    enabled: bool,
) -> Result<Reminder, String> {
    let mut reminders = repository
        .reminders
        .lock()
        .map_err(|error| error.to_string())?;
    let reminder = reminders
        .iter_mut()
        .find(|reminder| reminder.id == id)
        .ok_or_else(|| "Reminder not found".to_string())?;
    reminder.enabled = enabled;
    if enabled {
        let interval = reminder_interval_millis(reminder.interval_value, &reminder.interval_unit)?;
        reminder.next_run_at = current_time_millis().saturating_add(interval);
    }
    let updated = reminder.clone();
    persist_reminders(&repository, &reminders)?;
    Ok(updated)
}

#[tauri::command]
fn set_reminder_animation(
    repository: State<'_, ReminderRepository>,
    id: String,
    animation: String,
) -> Result<Reminder, String> {
    let mut reminders = repository
        .reminders
        .lock()
        .map_err(|error| error.to_string())?;
    let reminder = reminders
        .iter_mut()
        .find(|reminder| reminder.id == id)
        .ok_or_else(|| "Reminder not found".to_string())?;
    reminder.animation = normalize_reminder_animation(&animation);
    let updated = reminder.clone();
    persist_reminders(&repository, &reminders)?;
    Ok(updated)
}

#[tauri::command]
fn delete_reminder(repository: State<'_, ReminderRepository>, id: String) -> Result<(), String> {
    let mut reminders = repository
        .reminders
        .lock()
        .map_err(|error| error.to_string())?;
    let original_length = reminders.len();
    reminders.retain(|reminder| reminder.id != id);
    if reminders.len() == original_length {
        return Err("Reminder not found".to_string());
    }
    persist_reminders(&repository, &reminders)
}

#[tauri::command]
fn trigger_reminder_now(
    app: AppHandle,
    repository: State<'_, ReminderRepository>,
    pet_settings: State<'_, PetSettingsRepository>,
    id: String,
) -> Result<(), String> {
    let reminder = repository
        .reminders
        .lock()
        .map_err(|error| error.to_string())?
        .iter()
        .find(|reminder| reminder.id == id)
        .cloned()
        .ok_or_else(|| "Reminder not found".to_string())?;
    let settings = pet_settings
        .settings
        .lock()
        .map(|settings| settings.clone())
        .unwrap_or_else(|_| default_pet_settings());
    let sound_cue_id =
        (settings.sound_enabled && reminder.sound_enabled).then_some(reminder.sound_cue_id);
    show_scheduled_reminder(
        app,
        reminder.title,
        reminder.message,
        reminder.animation,
        reminder.visible_for_seconds,
        sound_cue_id,
        settings.sound_volume,
        settings.pet_size,
        settings.bubble_style,
    );
    Ok(())
}

#[cfg(target_os = "macos")]
#[cfg(target_pointer_width = "32")]
type CGFloat = f32;
#[cfg(target_os = "macos")]
#[cfg(target_pointer_width = "64")]
type CGFloat = f64;

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: CGFloat,
    y: CGFloat,
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: CGFloat,
    height: CGFloat,
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct CGColor {
    _private: [u8; 0],
}

#[cfg(target_os = "macos")]
unsafe impl objc2::encode::Encode for CGPoint {
    const ENCODING: objc2::encode::Encoding =
        objc2::encode::Encoding::Struct("CGPoint", &[CGFloat::ENCODING, CGFloat::ENCODING]);
}

#[cfg(target_os = "macos")]
unsafe impl objc2::encode::Encode for CGSize {
    const ENCODING: objc2::encode::Encoding =
        objc2::encode::Encoding::Struct("CGSize", &[CGFloat::ENCODING, CGFloat::ENCODING]);
}

#[cfg(target_os = "macos")]
unsafe impl objc2::encode::Encode for CGRect {
    const ENCODING: objc2::encode::Encoding =
        objc2::encode::Encoding::Struct("CGRect", &[CGPoint::ENCODING, CGSize::ENCODING]);
}

#[cfg(target_os = "macos")]
unsafe impl objc2::encode::Encode for CGColor {
    const ENCODING: objc2::encode::Encoding = objc2::encode::Encoding::Struct("CGColor", &[]);
}

#[cfg(target_os = "macos")]
unsafe impl objc2::encode::RefEncode for CGColor {
    const ENCODING_REF: objc2::encode::Encoding =
        objc2::encode::Encoding::Pointer(&<Self as objc2::encode::Encode>::ENCODING);
}

#[cfg(target_os = "macos")]
impl CGRect {
    fn new(x: CGFloat, y: CGFloat, width: CGFloat, height: CGFloat) -> Self {
        Self {
            origin: CGPoint { x, y },
            size: CGSize { width, height },
        }
    }
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum BubbleSide {
    Left,
    Right,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
struct PetLayout {
    panel_width: CGFloat,
    panel_height: CGFloat,
    image_x: CGFloat,
    image_y: CGFloat,
    image_size: CGFloat,
    bubble_x: CGFloat,
    bubble_y: CGFloat,
    bubble_height: CGFloat,
    bubble_side: BubbleSide,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
struct BubbleTheme {
    background: (CGFloat, CGFloat, CGFloat, CGFloat),
    ink: (CGFloat, CGFloat, CGFloat, CGFloat),
    shadow: (CGFloat, CGFloat, CGFloat, CGFloat),
    close_text: (CGFloat, CGFloat, CGFloat, CGFloat),
}

#[cfg(target_os = "macos")]
fn bubble_theme(style: &str, dark_mode: bool) -> BubbleTheme {
    if dark_mode {
        let background = match style {
            "pink" => (0.55, 0.22, 0.43, 1.0),
            "yellow" => (0.45, 0.36, 0.07, 1.0),
            "cyan" => (0.09, 0.38, 0.48, 1.0),
            _ => (0.28, 0.41, 0.11, 1.0),
        };

        BubbleTheme {
            background,
            ink: (0.96, 0.94, 1.0, 1.0),
            shadow: (0.01, 0.01, 0.02, 1.0),
            close_text: (0.09, 0.08, 0.11, 1.0),
        }
    } else {
        let background = match style {
            "pink" => (1.0, 0.47, 0.78, 1.0),
            "yellow" => (1.0, 0.85, 0.30, 1.0),
            "cyan" => (0.40, 0.87, 0.99, 1.0),
            _ => (0.75, 1.0, 0.30, 1.0),
        };

        BubbleTheme {
            background,
            ink: (0.09, 0.08, 0.11, 1.0),
            shadow: (0.09, 0.08, 0.11, 1.0),
            close_text: (1.0, 1.0, 1.0, 1.0),
        }
    }
}

#[cfg(target_os = "macos")]
impl PetLayout {
    fn new(pet_size: u32, bubble_side: BubbleSide, bubble_height: CGFloat) -> Self {
        let image_size = pet_size.clamp(MIN_PET_SIZE, MAX_PET_SIZE) as CGFloat;
        let image_x = match bubble_side {
            BubbleSide::Left => BUBBLE_WIDTH + 10.0,
            BubbleSide::Right => 10.0,
        };
        let image_y = 6.0;
        let bubble_x = match bubble_side {
            BubbleSide::Left => 10.0,
            BubbleSide::Right => image_size,
        };
        let bubble_y = image_y + image_size * 0.47;
        let panel_width = image_size + BUBBLE_WIDTH + 20.0;
        let panel_height = (image_y + image_size + 32.0).max(bubble_y + bubble_height + 20.0);

        Self {
            panel_width,
            panel_height,
            image_x,
            image_y,
            image_size,
            bubble_x,
            bubble_y,
            bubble_height,
            bubble_side,
        }
    }
}

#[cfg(target_os = "macos")]
#[derive(Clone)]
struct CharacterFramePaths {
    idle: Vec<PathBuf>,
    run_left: Vec<PathBuf>,
    run_right: Vec<PathBuf>,
    hover: Vec<PathBuf>,
    custom: Vec<(String, Vec<PathBuf>)>,
}

#[cfg(target_os = "macos")]
#[derive(Clone)]
struct CharacterFrameHandles {
    idle: Arc<Vec<usize>>,
    run_left: Arc<Vec<usize>>,
    run_right: Arc<Vec<usize>>,
    hover: Arc<Vec<usize>>,
    custom: Arc<Vec<(String, Vec<usize>)>>,
}

#[cfg(target_os = "macos")]
struct NativePet {
    panel: usize,
    retained_images: Vec<usize>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

#[cfg(target_os = "macos")]
struct NativePetPanelInfo {
    panel: usize,
    image_view: usize,
    bubble_views: Arc<Vec<usize>>,
    frames: CharacterFrameHandles,
    retained_images: Vec<usize>,
    layout: PetLayout,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum AnimationSlot {
    Idle,
    RunLeft,
    RunRight,
    Hover,
    Custom(usize),
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum PetMotion {
    Idle,
    RunLeft,
    RunRight,
    Hover,
    Reminder,
}

#[cfg(target_os = "macos")]
fn animation_slot_for_id(frames: &CharacterFrameHandles, animation_id: &str) -> AnimationSlot {
    match animation_id {
        "run-left" => AnimationSlot::RunLeft,
        "run-right" => AnimationSlot::RunRight,
        "hover" => AnimationSlot::Hover,
        "idle" => AnimationSlot::Idle,
        custom_id => frames
            .custom
            .iter()
            .position(|(id, _)| id == custom_id)
            .map(AnimationSlot::Custom)
            .unwrap_or(AnimationSlot::Idle),
    }
}

#[cfg(target_os = "macos")]
fn animation_frames<'a>(
    frames: &'a CharacterFrameHandles,
    animation: AnimationSlot,
) -> &'a [usize] {
    match animation {
        AnimationSlot::Idle => frames.idle.as_slice(),
        AnimationSlot::RunLeft => frames.run_left.as_slice(),
        AnimationSlot::RunRight => frames.run_right.as_slice(),
        AnimationSlot::Hover => frames.hover.as_slice(),
        AnimationSlot::Custom(index) => frames
            .custom
            .get(index)
            .map(|(_, images)| images.as_slice())
            .unwrap_or_else(|| frames.idle.as_slice()),
    }
}

#[cfg(target_os = "macos")]
struct PetInteractionState {
    dragging: bool,
    previous_mouse_down: bool,
    drag_offset: CGPoint,
    press_origin: CGPoint,
    moved_during_press: bool,
    last_x: CGFloat,
    motion: PetMotion,
    frame_index: usize,
    animation_tick: u8,
    launched_at: Instant,
    bubble_show_delay: Duration,
    bubble_visible_duration: Option<Duration>,
    bubble_alpha: CGFloat,
    bubble_dismissed: bool,
    bubble_enabled: bool,
    reminder_animation: AnimationSlot,
}

#[cfg(target_os = "macos")]
struct NativePetContent {
    view: *mut objc2::runtime::AnyObject,
    image_view: usize,
    bubble_views: Arc<Vec<usize>>,
    frames: CharacterFrameHandles,
    retained_images: Vec<usize>,
}

#[cfg(target_os = "macos")]
static NATIVE_PET: Mutex<Option<NativePet>> = Mutex::new(None);
#[cfg(target_os = "macos")]
static NATIVE_SOUND: Mutex<Option<usize>> = Mutex::new(None);
#[cfg(target_os = "macos")]
static LAST_PET_POSITION: Mutex<Option<(CGFloat, CGFloat)>> = Mutex::new(None);
#[cfg(not(target_os = "macos"))]
static LAST_PET_POSITION: Mutex<Option<(i32, i32)>> = Mutex::new(None);

#[cfg(target_os = "macos")]
const BUBBLE_WIDTH: CGFloat = 364.0;
#[cfg(target_os = "macos")]
const MIN_BUBBLE_HEIGHT: CGFloat = 92.0;
#[cfg(target_os = "macos")]
const MAX_BUBBLE_HEIGHT: CGFloat = 282.0;
#[cfg(target_os = "macos")]
const BUBBLE_CLOSE_SIZE: CGFloat = 24.0;
#[cfg(target_os = "macos")]
const BUBBLE_CLOSE_MARGIN: CGFloat = 8.0;
#[cfg(target_os = "macos")]
const BUBBLE_FADE_STEP: CGFloat = 0.065;

#[cfg(target_os = "macos")]
fn native_bubble_height(message: &str) -> CGFloat {
    let wrapped_lines = message
        .lines()
        .map(|line| {
            line.chars()
                .map(|character| if character.is_ascii() { 1 } else { 2 })
                .sum::<usize>()
                .max(1)
                .div_ceil(42)
        })
        .sum::<usize>()
        .clamp(1, 12);
    (54.0 + wrapped_lines as CGFloat * 19.0).clamp(MIN_BUBBLE_HEIGHT, MAX_BUBBLE_HEIGHT)
}

#[cfg(target_os = "macos")]
fn play_native_sound(app: &AppHandle, sound_cue_id: &str, volume: u8) -> Result<(), String> {
    let (bytes, _) = sound_cue_bytes(app, sound_cue_id)?;
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || unsafe {
        use objc2::msg_send;
        use objc2::runtime::{AnyClass, AnyObject, Bool};

        let result = (|| {
            let data_class =
                AnyClass::get("NSData").ok_or_else(|| "NSData not found".to_string())?;
            let data: *mut AnyObject = msg_send![
                data_class,
                dataWithBytes: bytes.as_ptr(),
                length: bytes.len()
            ];
            let sound_class =
                AnyClass::get("NSSound").ok_or_else(|| "NSSound not found".to_string())?;
            let sound_alloc: *mut AnyObject = msg_send![sound_class, alloc];
            let sound: *mut AnyObject = msg_send![sound_alloc, initWithData: data];
            if sound.is_null() {
                return Err("macOS could not decode this sound cue".to_string());
            }

            let _: () = msg_send![sound, setVolume: volume.min(100) as f32 / 100.0];
            let played: Bool = msg_send![sound, play];
            if played.is_false() {
                let _: () = msg_send![sound, release];
                return Err("macOS could not play this sound cue".to_string());
            }

            if let Ok(mut active_sound) = NATIVE_SOUND.lock() {
                if let Some(previous) = active_sound.replace(sound as usize) {
                    let previous = previous as *mut AnyObject;
                    let _: () = msg_send![previous, stop];
                    let _: () = msg_send![previous, release];
                }
            }
            Ok(())
        })();
        let _ = tx.send(result);
    })
    .map_err(|error| error.to_string())?;

    rx.recv_timeout(Duration::from_secs(2))
        .map_err(|_| "Timed out starting the sound cue".to_string())?
}

#[cfg(target_os = "macos")]
fn show_native_pet(
    app: AppHandle,
    title: String,
    message: String,
    show_after_seconds: u32,
    visible_for_seconds: u32,
    show_bubble: bool,
    pet_size: u32,
    bubble_style: String,
    reminder_animation: Option<String>,
) -> Result<(), String> {
    hide_native_pet(&app)?;

    let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);

    let frame_paths = resolve_character_frames(&app)?;
    let title: String = title.replace('\0', "").trim().chars().take(48).collect();
    let title = if title.is_empty() {
        "Baalert reminder".to_string()
    } else {
        title
    };
    let message = sanitize_message(message);
    let dark_mode = app
        .state::<PetSettingsRepository>()
        .settings
        .lock()
        .map(|settings| settings.dark_mode)
        .unwrap_or(false);
    let theme = bubble_theme(&bubble_style, dark_mode);
    let info =
        create_native_pet_on_main_thread(&app, frame_paths, title, message, pet_size, theme)?;
    let layout = info.layout;
    let reminder_animation = reminder_animation
        .as_deref()
        .map(|id| animation_slot_for_id(&info.frames, id))
        .unwrap_or(AnimationSlot::Idle);
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = stop.clone();
    let worker_app = app.clone();
    let interaction = Arc::new(Mutex::new(PetInteractionState {
        dragging: false,
        previous_mouse_down: false,
        drag_offset: CGPoint { x: 0.0, y: 0.0 },
        press_origin: CGPoint { x: 0.0, y: 0.0 },
        moved_during_press: false,
        last_x: 0.0,
        motion: PetMotion::Idle,
        frame_index: 0,
        animation_tick: 0,
        launched_at: Instant::now(),
        bubble_show_delay: Duration::from_secs(show_after_seconds.min(3600) as u64),
        bubble_visible_duration: (visible_for_seconds > 0).then(|| {
            Duration::from_secs(normalize_reminder_visible_for_seconds(visible_for_seconds) as u64)
        }),
        bubble_alpha: 0.0,
        bubble_dismissed: false,
        bubble_enabled: show_bubble,
        reminder_animation,
    }));
    let panel = info.panel;
    let image_view = info.image_view;
    let bubble_views = info.bubble_views.clone();
    let frames = info.frames.clone();

    let worker = std::thread::spawn(move || {
        while !worker_stop.load(Ordering::Acquire) {
            let stop_for_ui = worker_stop.clone();
            let interaction_for_ui = interaction.clone();
            let frames_for_ui = frames.clone();
            let bubble_views_for_ui = bubble_views.clone();
            let app_for_ui = worker_app.clone();

            let _ = worker_app.run_on_main_thread(move || {
                if stop_for_ui.load(Ordering::Acquire) {
                    return;
                }

                unsafe {
                    if update_pet_interaction(
                        panel,
                        image_view,
                        &bubble_views_for_ui,
                        &frames_for_ui,
                        &interaction_for_ui,
                        layout,
                    ) {
                        show_main_dashboard(&app_for_ui);
                    }
                }
            });

            std::thread::sleep(Duration::from_millis(16));
        }
    });

    *NATIVE_PET.lock().map_err(|e| e.to_string())? = Some(NativePet {
        panel,
        retained_images: info.retained_images,
        stop,
        worker: Some(worker),
    });

    Ok(())
}

#[cfg(target_os = "macos")]
fn hide_native_pet(app: &AppHandle) -> Result<(), String> {
    let current = NATIVE_PET.lock().map_err(|e| e.to_string())?.take();

    if let Some(mut pet) = current {
        pet.stop.store(true, Ordering::Release);
        if let Some(worker) = pet.worker.take() {
            let _ = worker.join();
        }

        let (tx, rx) = std::sync::mpsc::channel();
        let panel = pet.panel;
        let retained_images = pet.retained_images;
        let runner = app.clone();

        runner
            .run_on_main_thread(move || {
                unsafe {
                    use objc2::msg_send;
                    use objc2::runtime::AnyObject;

                    let panel = panel as *mut AnyObject;
                    let nil = std::ptr::null_mut::<AnyObject>();
                    let _: () = msg_send![panel, orderOut: nil];
                    let _: () = msg_send![panel, close];
                    let _: () = msg_send![panel, release];
                    release_ns_images(&retained_images);
                }

                let _ = tx.send(());
            })
            .map_err(|e| e.to_string())?;

        rx.recv_timeout(Duration::from_secs(1))
            .map_err(|_| "Timed out closing native pet panel".to_string())?;
    }

    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);

    Ok(())
}

fn sanitize_message(message: String) -> String {
    let message: String = message.replace('\0', "").trim().chars().take(240).collect();

    if message.is_empty() {
        "You have something coming up soon.".to_string()
    } else {
        message
    }
}

#[cfg(target_os = "macos")]
fn resolve_character_frames(app: &AppHandle) -> Result<CharacterFramePaths, String> {
    let root = resolve_active_character_root(app)?;

    let custom_root = root.join("custom");
    let mut custom = if custom_root.is_dir() {
        let mut directories = fs::read_dir(custom_root)
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect::<Vec<_>>();
        directories.sort();
        directories
            .into_iter()
            .filter_map(|directory| {
                let slug = directory.file_name()?.to_str()?.to_string();
                let frames = collect_png_frames(&directory).ok()?;
                Some((format!("custom/{slug}"), frames))
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    custom.retain(|(_, frames)| !frames.is_empty());

    let mut idle = collect_png_frames(&root.join("idle")).unwrap_or_default();
    let mut run_left = collect_png_frames(&root.join("run-left")).unwrap_or_default();
    let mut run_right = collect_png_frames(&root.join("run-right")).unwrap_or_default();
    let mut hover = collect_png_frames(&root.join("hover")).unwrap_or_default();
    let fallback = idle
        .first()
        .map(|_| idle.clone())
        .or_else(|| run_left.first().map(|_| run_left.clone()))
        .or_else(|| run_right.first().map(|_| run_right.clone()))
        .or_else(|| hover.first().map(|_| hover.clone()))
        .or_else(|| custom.first().map(|(_, frames)| frames.clone()))
        .ok_or_else(|| "The active character does not contain any animation frames".to_string())?;

    if idle.is_empty() {
        idle = fallback.clone();
    }
    if run_left.is_empty() {
        run_left = fallback.clone();
    }
    if run_right.is_empty() {
        run_right = fallback.clone();
    }
    if hover.is_empty() {
        hover = fallback;
    }

    Ok(CharacterFramePaths {
        idle,
        run_left,
        run_right,
        hover,
        custom,
    })
}

fn collect_png_frames(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let mut frames = fs::read_dir(directory)
        .map_err(|e| format!("Could not read {}: {e}", directory.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
        })
        .collect::<Vec<_>>();

    frames.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

    if frames.is_empty() {
        Err(format!("No PNG frames found in {}", directory.display()))
    } else {
        Ok(frames)
    }
}

#[cfg(target_os = "macos")]
fn create_native_pet_on_main_thread(
    app: &AppHandle,
    frame_paths: CharacterFramePaths,
    title: String,
    message: String,
    pet_size: u32,
    theme: BubbleTheme,
) -> Result<NativePetPanelInfo, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let runner = app.clone();

    runner
        .run_on_main_thread(move || {
            let result =
                unsafe { create_native_pet_panel(&frame_paths, &title, &message, pet_size, theme) };
            let _ = tx.send(result);
        })
        .map_err(|e| e.to_string())?;

    rx.recv_timeout(Duration::from_secs(2))
        .map_err(|_| "Timed out creating native pet panel".to_string())?
}

#[cfg(target_os = "macos")]
unsafe fn create_native_pet_panel(
    frame_paths: &CharacterFramePaths,
    title: &str,
    message: &str,
    pet_size: u32,
    theme: BubbleTheme,
) -> Result<NativePetPanelInfo, String> {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject, Bool};

    let screen_class = AnyClass::get("NSScreen").ok_or_else(|| "NSScreen not found".to_string())?;
    let screen: *mut AnyObject = msg_send![screen_class, mainScreen];
    if screen.is_null() {
        return Err("NSScreen.mainScreen is null".to_string());
    }

    let visible_frame: CGRect = msg_send![screen, visibleFrame];
    let bubble_height = native_bubble_height(message);
    let right_layout = PetLayout::new(pet_size, BubbleSide::Right, bubble_height);
    let left_layout = PetLayout::new(pet_size, BubbleSide::Left, bubble_height);
    let visible_right = visible_frame.origin.x + visible_frame.size.width;
    let default_pet_x = visible_right - right_layout.image_size - 32.0;
    let default_pet_y = visible_frame.origin.y + 48.0;
    let (pet_x, pet_y) = LAST_PET_POSITION
        .lock()
        .ok()
        .and_then(|position| *position)
        .unwrap_or((default_pet_x, default_pet_y));
    let right_origin_x = pet_x - right_layout.image_x;
    let left_origin_x = pet_x - left_layout.image_x;
    let layout = if right_origin_x + right_layout.panel_width <= visible_right
        || left_origin_x < visible_frame.origin.x
    {
        right_layout
    } else {
        left_layout
    };
    let initial_x = (pet_x - layout.image_x).clamp(
        visible_frame.origin.x,
        visible_frame.origin.x + visible_frame.size.width - layout.panel_width,
    );
    let initial_y = (pet_y - layout.image_y).clamp(
        visible_frame.origin.y,
        visible_frame.origin.y + visible_frame.size.height - layout.panel_height,
    );
    if let Ok(mut position) = LAST_PET_POSITION.lock() {
        *position = Some((initial_x + layout.image_x, initial_y + layout.image_y));
    }
    let panel_rect = CGRect::new(
        initial_x,
        initial_y,
        layout.panel_width,
        layout.panel_height,
    );

    let panel_class = AnyClass::get("NSPanel").ok_or_else(|| "NSPanel not found".to_string())?;
    let panel_alloc: *mut AnyObject = msg_send![panel_class, alloc];
    let style_mask: usize = 1usize << 7; // NSWindowStyleMaskNonactivatingPanel
    let panel: *mut AnyObject = msg_send![
        panel_alloc,
        initWithContentRect: panel_rect,
        styleMask: style_mask,
        backing: 2usize,
        defer: Bool::NO
    ];

    if panel.is_null() {
        return Err("Failed to create NSPanel".to_string());
    }

    let behavior: u64 = 1u64 // canJoinAllSpaces
        | 16u64 // stationary
        | 64u64 // ignoresCycle
        | 256u64 // fullScreenAuxiliary
        | (1u64 << 18); // canJoinAllApplications

    let clear = ns_color("clearColor");
    let _: () = msg_send![panel, setReleasedWhenClosed: Bool::NO];
    let _: () = msg_send![panel, setOpaque: Bool::NO];
    let _: () = msg_send![panel, setAlphaValue: 1.0 as CGFloat];
    let _: () = msg_send![panel, setBackgroundColor: clear];
    let _: () = msg_send![panel, setHasShadow: Bool::NO];
    let _: () = msg_send![panel, setIgnoresMouseEvents: Bool::YES];
    let _: () = msg_send![panel, setAcceptsMouseMovedEvents: Bool::YES];
    let _: () = msg_send![panel, setMovable: Bool::NO];
    let _: () = msg_send![panel, setCanHide: Bool::NO];
    let _: () = msg_send![panel, setHidesOnDeactivate: Bool::NO];
    let _: () = msg_send![panel, setCanBecomeVisibleWithoutLogin: Bool::YES];
    let _: () = msg_send![panel, setCollectionBehavior: behavior];
    let _: () = msg_send![panel, setLevel: macos_overlay_window_level()];
    let _: () = msg_send![panel, setWorksWhenModal: Bool::YES];
    let _: () = msg_send![panel, setBecomesKeyOnlyIfNeeded: Bool::YES];

    let content = match create_pet_content_view(frame_paths, title, message, layout, theme) {
        Ok(content) => content,
        Err(error) => {
            let _: () = msg_send![panel, release];
            return Err(error);
        }
    };
    let _: () = msg_send![panel, setContentView: content.view];
    let _: () = msg_send![content.view, release];
    let _: () = msg_send![panel, orderFrontRegardless];

    Ok(NativePetPanelInfo {
        panel: panel as usize,
        image_view: content.image_view,
        bubble_views: content.bubble_views,
        frames: content.frames,
        retained_images: content.retained_images,
        layout,
    })
}

#[cfg(target_os = "macos")]
unsafe fn create_pet_content_view(
    frame_paths: &CharacterFramePaths,
    title_text: &str,
    message: &str,
    layout: PetLayout,
    theme: BubbleTheme,
) -> Result<NativePetContent, String> {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject, Bool};

    let idle = load_ns_images(&frame_paths.idle)?;
    let run_left = match load_ns_images(&frame_paths.run_left) {
        Ok(images) => images,
        Err(error) => {
            release_ns_images(&idle);
            return Err(error);
        }
    };
    let run_right = match load_ns_images(&frame_paths.run_right) {
        Ok(images) => images,
        Err(error) => {
            release_ns_images(&idle);
            release_ns_images(&run_left);
            return Err(error);
        }
    };
    let hover = match load_ns_images(&frame_paths.hover) {
        Ok(images) => images,
        Err(error) => {
            release_ns_images(&idle);
            release_ns_images(&run_left);
            release_ns_images(&run_right);
            return Err(error);
        }
    };
    let mut custom = Vec::with_capacity(frame_paths.custom.len());
    for (animation_id, animation_paths) in &frame_paths.custom {
        match load_ns_images(animation_paths) {
            Ok(images) => custom.push((animation_id.clone(), images)),
            Err(error) => {
                release_ns_images(&idle);
                release_ns_images(&run_left);
                release_ns_images(&run_right);
                release_ns_images(&hover);
                for (_, images) in &custom {
                    release_ns_images(images);
                }
                return Err(error);
            }
        }
    }

    let custom_frame_count = custom.iter().map(|(_, images)| images.len()).sum::<usize>();
    let mut retained_images = Vec::with_capacity(
        idle.len() + run_left.len() + run_right.len() + hover.len() + custom_frame_count,
    );
    retained_images.extend(idle.iter().copied());
    retained_images.extend(run_left.iter().copied());
    retained_images.extend(run_right.iter().copied());
    retained_images.extend(hover.iter().copied());
    for (_, images) in &custom {
        retained_images.extend(images.iter().copied());
    }
    let frames = CharacterFrameHandles {
        idle: Arc::new(idle),
        run_left: Arc::new(run_left),
        run_right: Arc::new(run_right),
        hover: Arc::new(hover),
        custom: Arc::new(custom),
    };

    let view_class = AnyClass::get("NSView").ok_or_else(|| "NSView not found".to_string())?;
    let view_alloc: *mut AnyObject = msg_send![view_class, alloc];
    let view: *mut AnyObject = msg_send![
        view_alloc,
        initWithFrame: CGRect::new(0.0, 0.0, layout.panel_width, layout.panel_height)
    ];
    if view.is_null() {
        release_ns_images(&retained_images);
        return Err("Failed to create pet content view".to_string());
    }

    let _: () = msg_send![view, setWantsLayer: Bool::YES];
    let root_layer: *mut AnyObject = msg_send![view, layer];
    let clear_cg: *mut CGColor = msg_send![ns_color("clearColor"), CGColor];
    let _: () = msg_send![root_layer, setBackgroundColor: clear_cg];

    let image_view_class =
        AnyClass::get("NSImageView").ok_or_else(|| "NSImageView not found".to_string())?;
    let image_view_alloc: *mut AnyObject = msg_send![image_view_class, alloc];
    let image_view: *mut AnyObject = msg_send![
        image_view_alloc,
        initWithFrame: CGRect::new(
            layout.image_x,
            layout.image_y,
            layout.image_size,
            layout.image_size
        )
    ];
    if image_view.is_null() {
        let _: () = msg_send![view, release];
        release_ns_images(&retained_images);
        return Err("Failed to create pet image view".to_string());
    }

    let first_idle = frames.idle[0] as *mut AnyObject;
    let _: () = msg_send![image_view, setImage: first_idle];
    let _: () = msg_send![image_view, setImageScaling: 3usize];
    let _: () = msg_send![image_view, setAnimates: Bool::NO];
    let _: () = msg_send![image_view, setEnabled: Bool::YES];
    let _: () = msg_send![image_view, setAlphaValue: 1.0 as CGFloat];
    let _: () = msg_send![image_view, setWantsLayer: Bool::YES];
    let image_layer: *mut AnyObject = msg_send![image_view, layer];
    let _: () = msg_send![image_layer, setMagnificationFilter: ns_string("nearest")];
    let _: () = msg_send![image_layer, setMinificationFilter: ns_string("nearest")];
    let _: () = msg_send![view, addSubview: image_view];
    let image_view_handle = image_view as usize;
    let _: () = msg_send![image_view, release];

    let (large_dot_x, small_dot_x) = match layout.bubble_side {
        BubbleSide::Left => (
            layout.bubble_x + BUBBLE_WIDTH,
            layout.bubble_x + BUBBLE_WIDTH - 4.0,
        ),
        BubbleSide::Right => (layout.bubble_x - 13.0, layout.bubble_x - 4.0),
    };
    let large_thought_dot = add_thought_dot(
        view,
        CGRect::new(large_dot_x, layout.bubble_y, 13.0, 13.0),
        1.0,
        theme,
    )?;
    let small_thought_dot = add_thought_dot(
        view,
        CGRect::new(small_dot_x, layout.bubble_y + 13.0, 8.0, 8.0),
        1.0,
        theme,
    )?;

    let bubble_alloc: *mut AnyObject = msg_send![view_class, alloc];
    let bubble: *mut AnyObject = msg_send![
        bubble_alloc,
        initWithFrame: CGRect::new(
            layout.bubble_x,
            layout.bubble_y,
            BUBBLE_WIDTH,
            layout.bubble_height
        )
    ];
    if bubble.is_null() {
        let _: () = msg_send![view, release];
        release_ns_images(&retained_images);
        return Err("Failed to create reminder bubble".to_string());
    }

    let _: () = msg_send![bubble, setWantsLayer: Bool::YES];
    let bubble_layer: *mut AnyObject = msg_send![bubble, layer];
    let (red, green, blue, alpha) = theme.background;
    let (ink_red, ink_green, ink_blue, ink_alpha) = theme.ink;
    let ink = ns_color_with_rgba(ink_red, ink_green, ink_blue, ink_alpha);
    let (shadow_red, shadow_green, shadow_blue, shadow_alpha) = theme.shadow;
    let shadow = ns_color_with_rgba(shadow_red, shadow_green, shadow_blue, shadow_alpha);
    let background_cg: *mut CGColor =
        msg_send![ns_color_with_rgba(red, green, blue, alpha), CGColor];
    let border_cg: *mut CGColor = msg_send![ink, CGColor];
    let shadow_cg: *mut CGColor = msg_send![shadow, CGColor];
    let _: () = msg_send![bubble_layer, setBackgroundColor: background_cg];
    let _: () = msg_send![bubble_layer, setCornerRadius: 7.0 as CGFloat];
    let _: () = msg_send![bubble_layer, setBorderWidth: 3.0 as CGFloat];
    let _: () = msg_send![bubble_layer, setBorderColor: border_cg];
    let _: () = msg_send![bubble_layer, setShadowColor: shadow_cg];
    let _: () = msg_send![bubble_layer, setShadowOpacity: 1.0f32];
    let _: () = msg_send![bubble_layer, setShadowRadius: 0.0 as CGFloat];
    let _: () = msg_send![bubble_layer, setShadowOffset: CGSize { width: 6.0, height: -6.0 }];
    let _: () = msg_send![bubble_layer, setMasksToBounds: Bool::NO];

    let title = create_pet_text_field(
        CGRect::new(20.0, layout.bubble_height - 33.0, BUBBLE_WIDTH - 78.0, 18.0),
        title_text,
        12.0,
        true,
        ink,
    )?;
    let text = create_pet_text_field(
        CGRect::new(20.0, 12.0, BUBBLE_WIDTH - 40.0, layout.bubble_height - 48.0),
        message,
        15.0,
        true,
        ink,
    )?;
    let text_cell: *mut AnyObject = msg_send![text, cell];
    let _: () = msg_send![text_cell, setWraps: Bool::YES];
    let _: () = msg_send![text_cell, setScrollable: Bool::NO];
    let _: () = msg_send![text_cell, setUsesSingleLineMode: Bool::NO];
    let _: () = msg_send![text_cell, setLineBreakMode: 0usize];

    let close_x = BUBBLE_WIDTH - BUBBLE_CLOSE_MARGIN - BUBBLE_CLOSE_SIZE;
    let close_y = layout.bubble_height - BUBBLE_CLOSE_MARGIN - BUBBLE_CLOSE_SIZE;
    let close_alloc: *mut AnyObject = msg_send![view_class, alloc];
    let close_view: *mut AnyObject = msg_send![
        close_alloc,
        initWithFrame: CGRect::new(close_x, close_y, BUBBLE_CLOSE_SIZE, BUBBLE_CLOSE_SIZE)
    ];
    if close_view.is_null() {
        let _: () = msg_send![title, release];
        let _: () = msg_send![text, release];
        let _: () = msg_send![bubble, release];
        let _: () = msg_send![view, release];
        release_ns_images(&retained_images);
        return Err("Failed to create bubble close control".to_string());
    }
    let _: () = msg_send![close_view, setWantsLayer: Bool::YES];
    let close_layer: *mut AnyObject = msg_send![close_view, layer];
    let close_background: *mut CGColor = msg_send![ink, CGColor];
    let _: () = msg_send![close_layer, setBackgroundColor: close_background];
    let _: () = msg_send![close_layer, setCornerRadius: 4.0 as CGFloat];
    let (close_red, close_green, close_blue, close_alpha) = theme.close_text;
    let close_label = create_pet_text_field(
        CGRect::new(5.0, 3.0, BUBBLE_CLOSE_SIZE - 10.0, BUBBLE_CLOSE_SIZE - 6.0),
        "x",
        12.0,
        true,
        ns_color_with_rgba(close_red, close_green, close_blue, close_alpha),
    )?;
    let _: () = msg_send![close_label, setAlignment: 1isize];
    let _: () = msg_send![close_view, addSubview: close_label];
    let _: () = msg_send![bubble, addSubview: close_view];
    let _: () = msg_send![close_label, release];
    let _: () = msg_send![close_view, release];

    let _: () = msg_send![bubble, addSubview: title];
    let _: () = msg_send![bubble, addSubview: text];
    let _: () = msg_send![view, addSubview: bubble];
    let bubble_handle = bubble as usize;
    let _: () = msg_send![title, release];
    let _: () = msg_send![text, release];
    let _: () = msg_send![bubble, release];

    let bubble_views = Arc::new(vec![bubble_handle, large_thought_dot, small_thought_dot]);
    for bubble_view in bubble_views.iter() {
        let bubble_view = *bubble_view as *mut AnyObject;
        let _: () = msg_send![bubble_view, setAlphaValue: 0.0 as CGFloat];
        let _: () = msg_send![bubble_view, setHidden: Bool::YES];
    }

    Ok(NativePetContent {
        view,
        image_view: image_view_handle,
        bubble_views,
        frames,
        retained_images,
    })
}

#[cfg(target_os = "macos")]
unsafe fn load_ns_images(paths: &[PathBuf]) -> Result<Vec<usize>, String> {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};

    let image_class = AnyClass::get("NSImage").ok_or_else(|| "NSImage not found".to_string())?;
    let mut images = Vec::with_capacity(paths.len());

    for path in paths {
        let image_alloc: *mut AnyObject = msg_send![image_class, alloc];
        let image: *mut AnyObject = msg_send![
            image_alloc,
            initWithContentsOfFile: ns_string(path.to_string_lossy().as_ref())
        ];
        if image.is_null() {
            release_ns_images(&images);
            return Err(format!("Failed to load character frame {}", path.display()));
        }
        images.push(image as usize);
    }

    Ok(images)
}

#[cfg(target_os = "macos")]
unsafe fn release_ns_images(images: &[usize]) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;

    for image in images {
        let image = *image as *mut AnyObject;
        let _: () = msg_send![image, release];
    }
}

#[cfg(target_os = "macos")]
unsafe fn add_thought_dot(
    parent: *mut objc2::runtime::AnyObject,
    frame: CGRect,
    opacity: CGFloat,
    theme: BubbleTheme,
) -> Result<usize, String> {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject, Bool};

    let view_class = AnyClass::get("NSView").ok_or_else(|| "NSView not found".to_string())?;
    let dot_alloc: *mut AnyObject = msg_send![view_class, alloc];
    let dot: *mut AnyObject = msg_send![dot_alloc, initWithFrame: frame];
    if dot.is_null() {
        return Err("Failed to create thought bubble dot".to_string());
    }

    let _: () = msg_send![dot, setWantsLayer: Bool::YES];
    let layer: *mut AnyObject = msg_send![dot, layer];
    let (red, green, blue, alpha) = theme.background;
    let color: *mut CGColor = msg_send![
        ns_color_with_rgba(red, green, blue, alpha * opacity),
        CGColor
    ];
    let (ink_red, ink_green, ink_blue, ink_alpha) = theme.ink;
    let border: *mut CGColor = msg_send![
        ns_color_with_rgba(ink_red, ink_green, ink_blue, ink_alpha),
        CGColor
    ];
    let _: () = msg_send![layer, setBackgroundColor: color];
    let _: () = msg_send![layer, setCornerRadius: frame.size.width / 2.0];
    let _: () = msg_send![layer, setBorderWidth: 2.0 as CGFloat];
    let _: () = msg_send![layer, setBorderColor: border];
    let _: () = msg_send![parent, addSubview: dot];
    let dot_handle = dot as usize;
    let _: () = msg_send![dot, release];
    Ok(dot_handle)
}

#[cfg(target_os = "macos")]
unsafe fn create_pet_text_field(
    frame: CGRect,
    value: &str,
    font_size: CGFloat,
    bold: bool,
    color: *mut objc2::runtime::AnyObject,
) -> Result<*mut objc2::runtime::AnyObject, String> {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject, Bool};

    let text_field_class =
        AnyClass::get("NSTextField").ok_or_else(|| "NSTextField not found".to_string())?;
    let field_alloc: *mut AnyObject = msg_send![text_field_class, alloc];
    let field: *mut AnyObject = msg_send![field_alloc, initWithFrame: frame];
    if field.is_null() {
        return Err("Failed to create reminder text field".to_string());
    }

    let font_class = AnyClass::get("NSFont").ok_or_else(|| "NSFont not found".to_string())?;
    let font: *mut AnyObject = if bold {
        msg_send![font_class, boldSystemFontOfSize: font_size]
    } else {
        msg_send![font_class, systemFontOfSize: font_size]
    };
    let _: () = msg_send![field, setStringValue: ns_string(value)];
    let _: () = msg_send![field, setFont: font];
    let _: () = msg_send![field, setTextColor: color];
    let _: () = msg_send![field, setBezeled: Bool::NO];
    let _: () = msg_send![field, setBordered: Bool::NO];
    let _: () = msg_send![field, setEditable: Bool::NO];
    let _: () = msg_send![field, setSelectable: Bool::NO];
    let _: () = msg_send![field, setDrawsBackground: Bool::NO];
    let _: () = msg_send![field, setAlignment: 0isize];
    Ok(field)
}

#[cfg(target_os = "macos")]
unsafe fn update_pet_interaction(
    panel: usize,
    image_view: usize,
    bubble_views: &Arc<Vec<usize>>,
    frames: &CharacterFrameHandles,
    interaction: &Arc<Mutex<PetInteractionState>>,
    layout: PetLayout,
) -> bool {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject, Bool};

    let Some(event_class) = AnyClass::get("NSEvent") else {
        return false;
    };
    let panel = panel as *mut AnyObject;
    let image_view = image_view as *mut AnyObject;
    let mouse_buttons: usize = msg_send![event_class, pressedMouseButtons];
    let mouse_down = mouse_buttons & 1 == 1;
    let cursor: CGPoint = msg_send![event_class, mouseLocation];
    let panel_frame: CGRect = msg_send![panel, frame];
    let Ok(mut state) = interaction.lock() else {
        return false;
    };
    let clicked =
        state.dragging && state.previous_mouse_down && !mouse_down && !state.moved_during_press;

    let elapsed = state.launched_at.elapsed();
    let bubble_should_show = state.bubble_enabled
        && !state.bubble_dismissed
        && elapsed >= state.bubble_show_delay
        && state
            .bubble_visible_duration
            .is_none_or(|duration| elapsed < state.bubble_show_delay + duration);
    if bubble_should_show {
        state.bubble_alpha = (state.bubble_alpha + BUBBLE_FADE_STEP).min(1.0);
    } else {
        state.bubble_alpha = (state.bubble_alpha - BUBBLE_FADE_STEP).max(0.0);
    }
    let bubble_hidden = if state.bubble_alpha <= 0.0 {
        Bool::YES
    } else {
        Bool::NO
    };
    for bubble_view in bubble_views.iter() {
        let bubble_view = *bubble_view as *mut AnyObject;
        let _: () = msg_send![bubble_view, setHidden: bubble_hidden];
        let _: () = msg_send![bubble_view, setAlphaValue: state.bubble_alpha];
    }

    let local_x = cursor.x - panel_frame.origin.x;
    let local_y = cursor.y - panel_frame.origin.y;
    let over_pet = (layout.image_x..=layout.image_x + layout.image_size).contains(&local_x)
        && (layout.image_y..=layout.image_y + layout.image_size).contains(&local_y);
    let close_x = layout.bubble_x + BUBBLE_WIDTH - BUBBLE_CLOSE_MARGIN - BUBBLE_CLOSE_SIZE;
    let close_y = layout.bubble_y + layout.bubble_height - BUBBLE_CLOSE_MARGIN - BUBBLE_CLOSE_SIZE;
    let over_close = state.bubble_alpha > 0.05
        && (close_x..=close_x + BUBBLE_CLOSE_SIZE).contains(&local_x)
        && (close_y..=close_y + BUBBLE_CLOSE_SIZE).contains(&local_y);
    let ignores_mouse = if state.dragging || over_pet || over_close {
        Bool::NO
    } else {
        Bool::YES
    };
    let _: () = msg_send![panel, setIgnoresMouseEvents: ignores_mouse];

    if mouse_down && !state.previous_mouse_down {
        if over_close {
            state.bubble_dismissed = true;
        } else if over_pet {
            state.dragging = true;
            state.drag_offset = CGPoint {
                x: local_x,
                y: local_y,
            };
            state.press_origin = cursor;
            state.moved_during_press = false;
            state.last_x = panel_frame.origin.x;
        }
    }

    let reminder_visible = bubble_should_show || state.bubble_alpha > 0.05;
    let mut desired_motion = if reminder_visible {
        PetMotion::Reminder
    } else {
        PetMotion::Idle
    };
    if state.dragging && mouse_down {
        let total_delta_x = cursor.x - state.press_origin.x;
        let total_delta_y = cursor.y - state.press_origin.y;
        if total_delta_x.abs() >= 5.0 || total_delta_y.abs() >= 5.0 {
            state.moved_during_press = true;
        }

        let new_origin = CGPoint {
            x: cursor.x - state.drag_offset.x,
            y: cursor.y - state.drag_offset.y,
        };
        let delta_x = new_origin.x - state.last_x;
        let _: () = msg_send![panel, setFrameOrigin: new_origin];

        if delta_x < -0.25 {
            desired_motion = PetMotion::RunLeft;
        } else if delta_x > 0.25 {
            desired_motion = PetMotion::RunRight;
        } else if matches!(state.motion, PetMotion::RunLeft | PetMotion::RunRight) {
            desired_motion = state.motion;
        }

        state.last_x = new_origin.x;
        if let Ok(mut position) = LAST_PET_POSITION.lock() {
            *position = Some((new_origin.x + layout.image_x, new_origin.y + layout.image_y));
        }
    } else if !mouse_down {
        state.dragging = false;
        if !matches!(desired_motion, PetMotion::Reminder) {
            desired_motion = if over_pet {
                PetMotion::Hover
            } else {
                PetMotion::Idle
            };
        }
    }

    state.previous_mouse_down = mouse_down;
    if desired_motion != state.motion {
        state.motion = desired_motion;
        state.frame_index = 0;
        state.animation_tick = 0;
    } else {
        state.animation_tick += 1;
        if state.animation_tick >= 5 {
            state.animation_tick = 0;
            state.frame_index += 1;
        }
    }

    let active_frames: &[usize] = match state.motion {
        PetMotion::Idle => frames.idle.as_slice(),
        PetMotion::RunLeft => frames.run_left.as_slice(),
        PetMotion::RunRight => frames.run_right.as_slice(),
        PetMotion::Hover => frames.hover.as_slice(),
        PetMotion::Reminder => animation_frames(frames, state.reminder_animation),
    };
    state.frame_index %= active_frames.len();
    let image = active_frames[state.frame_index] as *mut AnyObject;
    let _: () = msg_send![image_view, setImage: image];
    let _: () = msg_send![panel, orderFrontRegardless];
    clicked
}

#[cfg(target_os = "macos")]
unsafe fn show_main_dashboard(app: &AppHandle) {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject, Bool};

    let Some(application_class) = AnyClass::get("NSApplication") else {
        return;
    };
    let application: *mut AnyObject = msg_send![application_class, sharedApplication];
    let _: Bool = msg_send![application, setActivationPolicy: 0isize];

    if let Some(dashboard) = app.get_webview_window("main") {
        if let Ok(window) = dashboard.ns_window() {
            let window = window as *mut AnyObject;
            let nil = std::ptr::null_mut::<AnyObject>();
            let _: () = msg_send![window, setAlphaValue: 1.0 as CGFloat];
            let _: () = msg_send![window, deminiaturize: nil];
            let _: () = msg_send![window, makeKeyAndOrderFront: nil];
            let _: () = msg_send![window, orderFrontRegardless];
        }
    }

    let _: () = msg_send![application, activateIgnoringOtherApps: Bool::YES];
}

#[cfg(target_os = "macos")]
unsafe fn ns_string(value: &str) -> *mut objc2::runtime::AnyObject {
    use objc2::msg_send;
    use objc2::runtime::AnyClass;

    let class = AnyClass::get("NSString").expect("NSString not found");
    let value = CString::new(value).expect("NSString source contained NUL");
    msg_send![class, stringWithUTF8String: value.as_ptr()]
}

#[cfg(target_os = "macos")]
unsafe fn ns_color(selector: &str) -> *mut objc2::runtime::AnyObject {
    use objc2::msg_send;
    use objc2::runtime::AnyClass;

    let class = AnyClass::get("NSColor").expect("NSColor not found");
    match selector {
        "clearColor" => msg_send![class, clearColor],
        "whiteColor" => msg_send![class, whiteColor],
        _ => msg_send![class, clearColor],
    }
}

#[cfg(target_os = "macos")]
unsafe fn ns_color_with_rgba(
    red: CGFloat,
    green: CGFloat,
    blue: CGFloat,
    alpha: CGFloat,
) -> *mut objc2::runtime::AnyObject {
    use objc2::msg_send;
    use objc2::runtime::AnyClass;

    let class = AnyClass::get("NSColor").expect("NSColor not found");
    msg_send![
        class,
        colorWithCalibratedRed: red,
        green: green,
        blue: blue,
        alpha: alpha
    ]
}

#[cfg(target_os = "macos")]
fn macos_overlay_window_level() -> isize {
    const K_CG_ASSISTIVE_TECH_HIGH_WINDOW_LEVEL_KEY: i32 = 20;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGWindowLevelForKey(key: i32) -> i32;
    }

    unsafe { CGWindowLevelForKey(K_CG_ASSISTIVE_TECH_HIGH_WINDOW_LEVEL_KEY) as isize }
}

#[cfg(target_os = "windows")]
fn windows_mci_command(command: &str) -> Result<(), String> {
    let command = command
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let result = unsafe { mciSendStringW(command.as_ptr(), std::ptr::null_mut(), 0, 0) };
    if result == 0 {
        Ok(())
    } else {
        Err(format!("Windows audio command failed with code {result}"))
    }
}

#[cfg(target_os = "windows")]
fn play_windows_sound(app: &AppHandle, sound_cue_id: &str, volume: u8) -> Result<(), String> {
    let path = sound_cue_path(app, sound_cue_id)?;
    let _ = windows_mci_command("close baalert_sound");
    windows_mci_command(&format!(
        "open \"{}\" alias baalert_sound",
        path.to_string_lossy()
    ))?;
    windows_mci_command(&format!(
        "setaudio baalert_sound volume to {}",
        volume.min(100) as u32 * 10
    ))?;
    windows_mci_command("play baalert_sound from 0")
}

#[tauri::command]
async fn show_pet(
    app: AppHandle,
    title: Option<String>,
    message: String,
    show_after_seconds: u32,
    visible_for_seconds: u32,
    show_bubble: bool,
    pet_size: u32,
    bubble_style: String,
    reminder_animation: Option<String>,
    sound_cue_id: Option<String>,
    sound_volume: Option<u8>,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        if let Some(sound_cue_id) = sound_cue_id.as_deref() {
            let _ = play_native_sound(&app, sound_cue_id, sound_volume.unwrap_or(70));
        }
        return show_native_pet(
            app,
            title.unwrap_or_else(|| "Baalert reminder".to_string()),
            message,
            show_after_seconds,
            visible_for_seconds,
            show_bubble,
            pet_size,
            bubble_style,
            reminder_animation,
        );
    }

    #[cfg(not(target_os = "macos"))]
    {
        #[cfg(target_os = "windows")]
        if let Some(sound_cue_id) = sound_cue_id.as_deref() {
            let _ = play_windows_sound(&app, sound_cue_id, sound_volume.unwrap_or(70));
        }

        #[cfg(target_os = "windows")]
        let sound_data_url = None;
        #[cfg(not(target_os = "windows"))]
        let sound_data_url = sound_cue_id
            .as_deref()
            .and_then(|id| get_sound_cue_data_url(app.clone(), id.to_string()).ok());
        let pet_size = pet_size.clamp(MIN_PET_SIZE, MAX_PET_SIZE);
        let title: String = title
            .unwrap_or_else(|| "Baalert reminder".to_string())
            .replace('\0', "")
            .trim()
            .chars()
            .take(48)
            .collect();
        let dark_mode = app
            .state::<PetSettingsRepository>()
            .settings
            .lock()
            .map(|settings| settings.dark_mode)
            .unwrap_or(false);
        let payload = PetOverlayPayload {
            revision: current_time_millis(),
            title: if title.is_empty() {
                "Baalert reminder".to_string()
            } else {
                title
            },
            message: sanitize_message(message),
            show_after_seconds: show_after_seconds.min(3600),
            visible_for_seconds: normalize_reminder_visible_for_seconds(visible_for_seconds),
            show_bubble,
            pet_size,
            bubble_style: normalize_bubble_style(&bubble_style),
            reminder_animation: reminder_animation
                .as_deref()
                .map(normalize_reminder_animation)
                .unwrap_or_else(default_reminder_animation),
            sound_data_url,
            sound_volume: sound_volume.unwrap_or(70).min(100),
            dark_mode,
        };

        if let Some(existing) = app.get_webview_window("pet") {
            existing
                .emit("pet-overlay-update", payload)
                .map_err(|error| error.to_string())?;
            existing
                .set_always_on_top(true)
                .map_err(|error| error.to_string())?;
            existing
                .set_visible_on_all_workspaces(true)
                .map_err(|error| error.to_string())?;
            existing.show().map_err(|error| error.to_string())?;
            return Ok(());
        }

        let previous_position = LAST_PET_POSITION.lock().ok().and_then(|position| *position);
        let payload_json = serde_json::to_vec(&payload).map_err(|error| error.to_string())?;
        let url = format!(
            "index.html?mode=pet&payload={}",
            BASE64_URL_SAFE_NO_PAD.encode(payload_json)
        );
        let (position_x, position_y) = previous_position.unwrap_or((40, 40));
        if let Ok(mut position) = LAST_PET_POSITION.lock() {
            *position = Some((position_x, position_y));
        }
        let pet_window = WebviewWindowBuilder::new(&app, "pet", WebviewUrl::App(url.into()))
            .title("")
            .decorations(false)
            .transparent(true)
            .always_on_top(true)
            .visible_on_all_workspaces(true)
            .skip_taskbar(true)
            .resizable(false)
            .focusable(false)
            .focused(false)
            .visible(false)
            .shadow(false)
            .inner_size((pet_size + 28) as f64, (pet_size + 28) as f64)
            .position(position_x as f64, position_y as f64)
            .build()
            .map_err(|e| e.to_string())?;

        pet_window
            .set_visible_on_all_workspaces(true)
            .map_err(|e| e.to_string())?;
        pet_window.show().map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[tauri::command]
fn open_dashboard(app: AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    unsafe {
        show_main_dashboard(&app);
    }

    #[cfg(not(target_os = "macos"))]
    if let Some(window) = app.get_webview_window("main") {
        window.show().map_err(|error| error.to_string())?;
        let _ = window.unminimize();
        window.set_focus().map_err(|error| error.to_string())?;
    }

    Ok(())
}

#[tauri::command]
fn is_primary_mouse_button_pressed() -> Option<bool> {
    #[cfg(target_os = "windows")]
    unsafe {
        const VK_LBUTTON: i32 = 0x01;
        return Some(GetAsyncKeyState(VK_LBUTTON) < 0);
    }

    #[cfg(not(target_os = "windows"))]
    None
}

#[tauri::command]
async fn hide_pet(app: AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        return hide_native_pet(&app);
    }

    #[cfg(not(target_os = "macos"))]
    {
        if let Some(window) = app.get_webview_window("pet") {
            if let Ok(position) = window.outer_position() {
                if let Ok(mut last_position) = LAST_PET_POSITION.lock() {
                    *last_position = Some((position.x, position.y));
                }
            }
            window.close().map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

#[tauri::command]
fn is_pet_visible(app: AppHandle) -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        return NATIVE_PET
            .lock()
            .map(|pet| pet.is_some())
            .map_err(|error| error.to_string());
    }

    #[cfg(not(target_os = "macos"))]
    {
        Ok(app.get_webview_window("pet").is_some())
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let repository =
                create_reminder_repository(app.handle()).map_err(std::io::Error::other)?;
            let pet_settings =
                create_pet_settings_repository(app.handle()).map_err(std::io::Error::other)?;
            app.manage(repository.clone());
            app.manage(pet_settings.clone());
            spawn_reminder_scheduler(app.handle().clone(), repository, pet_settings);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            show_pet,
            hide_pet,
            is_pet_visible,
            open_dashboard,
            is_primary_mouse_button_pressed,
            list_reminders,
            create_reminder,
            update_reminder,
            set_reminder_enabled,
            set_reminder_animation,
            delete_reminder,
            trigger_reminder_now,
            get_pet_settings,
            set_pet_size,
            set_bubble_style,
            set_dark_mode,
            set_sound_settings,
            list_sound_cues,
            import_sound_cue,
            delete_sound_cue,
            get_sound_cue_data_url,
            list_characters,
            create_character,
            add_character_animation,
            delete_character_animation,
            import_character,
            set_active_character,
            delete_character,
            get_animation_preview_frames,
            get_pet_overlay_frames
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_main_animation_slot_aliases() {
        assert_eq!(classify_animation("idle").unwrap().0, "idle");
        assert_eq!(classify_animation("run_left").unwrap().0, "run-left");
        assert_eq!(classify_animation("Run Right").unwrap().0, "run-right");
        assert_eq!(classify_animation("mouse_hover").unwrap().0, "hover");
    }

    #[test]
    fn accepts_named_custom_animation_folders() {
        let animation = classify_animation("animations/happy-dance").unwrap();
        assert_eq!(animation.0, "custom/happy-dance");
        assert_eq!(animation.1, "Happy Dance");
        assert_eq!(classify_animation("custom/idle").unwrap().0, "custom/idle");
    }

    #[test]
    fn rejects_character_path_traversal() {
        assert!(is_safe_character_id("my-character-123"));
        assert!(!is_safe_character_id("../settings"));
        assert!(!is_safe_character_id("folder/character"));
    }

    #[test]
    fn migrates_existing_reminders_to_idle_animation() {
        let reminder: Reminder = serde_json::from_str(
            r#"{
                "id": "1",
                "title": "Water",
                "message": "Drink water",
                "intervalValue": 30,
                "intervalUnit": "minutes",
                "enabled": true,
                "nextRunAt": 1000
            }"#,
        )
        .unwrap();
        assert_eq!(reminder.animation, "idle");
        assert_eq!(reminder.visible_for_seconds, 10);
        assert!(!reminder.sound_enabled);
        assert_eq!(reminder.sound_cue_id, DEFAULT_SOUND_CUE_ID);
    }

    #[test]
    fn migrates_existing_pet_settings_to_sound_defaults() {
        let settings: PetSettings = serde_json::from_str(
            r#"{
                "petSize": 152,
                "bubbleStyle": "pink",
                "activeCharacterId": "builtin-baalert",
                "darkMode": true
            }"#,
        )
        .unwrap();
        assert!(settings.sound_enabled);
        assert_eq!(settings.sound_volume, 70);
    }

    #[test]
    fn normalizes_sound_cue_ids() {
        assert_eq!(
            normalize_sound_cue_id("builtin-bright-pop"),
            "builtin-bright-pop"
        );
        assert_eq!(normalize_sound_cue_id("custom-123"), "custom-123");
        assert_eq!(normalize_sound_cue_id("../sound"), DEFAULT_SOUND_CUE_ID);
    }

    #[test]
    fn builds_valid_builtin_wav_cues() {
        for cue in builtin_sound_cues() {
            let bytes = builtin_sound_bytes(&cue.id).unwrap();
            assert!(bytes.starts_with(b"RIFF"));
            assert_eq!(bytes.get(8..12), Some(b"WAVE".as_slice()));
            assert!(bytes.len() > 44);
            assert!(validate_sound_file("cue.wav", &bytes).is_ok());
        }
    }

    #[test]
    fn rejects_invalid_local_sound_files() {
        assert!(validate_sound_file("cue.ogg", b"OggS").is_err());
        assert!(validate_sound_file("cue.mp3", b"not an mp3").is_err());
        assert!(validate_sound_file("cue.wav", b"RIFFbad data").is_err());
    }

    #[test]
    fn normalizes_reminder_bubble_duration() {
        assert_eq!(normalize_reminder_visible_for_seconds(0), 0);
        assert_eq!(normalize_reminder_visible_for_seconds(1), 2);
        assert_eq!(normalize_reminder_visible_for_seconds(30), 30);
        assert_eq!(normalize_reminder_visible_for_seconds(4_000), 3_600);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn expands_native_bubble_for_long_messages() {
        let short = native_bubble_height("Take a break.");
        let long = native_bubble_height(&"Long reminder text ".repeat(12));
        assert_eq!(short, MIN_BUBBLE_HEIGHT);
        assert!(long > short);
    }

    #[test]
    fn normalizes_custom_reminder_animation_ids() {
        assert_eq!(
            normalize_reminder_animation("custom/Happy Dance"),
            "custom/happy-dance"
        );
        assert_eq!(normalize_reminder_animation(""), "idle");
        assert_eq!(normalize_reminder_animation("hover"), "hover");
    }

    #[test]
    fn allows_partial_local_character_packs() {
        let root =
            std::env::temp_dir().join(format!("baalert-draft-character-{}", current_time_millis()));
        fs::create_dir_all(root.join("idle")).unwrap();
        fs::write(
            root.join("idle").join("0000.png"),
            [137, 80, 78, 71, 13, 10, 26, 10],
        )
        .unwrap();

        let pack = character_pack_from_root(
            &root,
            "draft-character".to_string(),
            "Draft character".to_string(),
            false,
        )
        .unwrap();

        assert!(pack.is_ready);
        assert_eq!(pack.animations.len(), 1);
        assert_eq!(pack.animations[0].id, "idle");
        let _ = fs::remove_dir_all(root);
    }
}
