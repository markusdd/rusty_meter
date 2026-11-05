//! Named settings profiles. Default keeps the original `rustymeter` folder.
//! Every other profile is a sibling folder, and a lock file keeps two windows
//! from opening the same one.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::agent_proto::DEFAULT_PORT;

pub const DEFAULT_NAME: &str = "Default";
const DEFAULT_SLUG: &str = "default";

enum Startup {
    Profile { display: String },
    Picker { notice: Option<String> },
}

static STARTUP: std::sync::OnceLock<Startup> = std::sync::OnceLock::new();
static HELD_LOCK: Mutex<Option<File>> = Mutex::new(None);

pub struct NativeLaunch {
    pub app_id: String,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedProfile {
    pub name: String,
    pub locked: bool,
}

/// Choose the settings folder and hold its lock before eframe starts.
pub fn resolve_launch() -> NativeLaunch {
    let startup = match requested_name() {
        Some(name) => match acquire_named(&name) {
            Ok(()) => Startup::Profile { display: name },
            Err(notice) => Startup::Picker {
                notice: Some(format!(
                    "{notice} This window starts from Default and stays disconnected."
                )),
            },
        },
        None => match acquire_slug(DEFAULT_SLUG) {
            Ok(()) => Startup::Profile {
                display: DEFAULT_NAME.to_owned(),
            },
            Err(_) => Startup::Picker {
                notice: Some(
                    "Default is open in another window. This window starts from those settings and stays disconnected. Choose another profile, or save this window as a new one."
                        .to_owned(),
                ),
            },
        },
    };
    let launch = match &startup {
        Startup::Profile { display } => NativeLaunch {
            app_id: app_id_for_slug(&slug_for(display).unwrap_or_else(|_| DEFAULT_SLUG.to_owned())),
            title: format!("RustyMeter — {display}"),
        },
        Startup::Picker { .. } => NativeLaunch {
            app_id: format!("RustyMeter-unsaved-{}", std::process::id()),
            title: "RustyMeter".to_owned(),
        },
    };
    let _ = STARTUP.set(startup);
    launch
}

pub fn startup_profile() -> Option<StartupView> {
    STARTUP.get().map(|startup| match startup {
        Startup::Profile { display } => StartupView::Profile {
            display: display.clone(),
        },
        Startup::Picker { notice } => StartupView::Picker {
            notice: notice.clone(),
        },
    })
}

pub enum StartupView {
    Profile { display: String },
    Picker { notice: Option<String> },
}

pub fn list_profiles() -> Vec<ListedProfile> {
    let Some(root) = data_root() else {
        return Vec::new();
    };
    list_profiles_in(&root)
}

pub fn write_new_profile(name: &str, app_ron: &str) -> Result<(), String> {
    let root = data_root().ok_or("could not find the settings folder")?;
    write_new_profile_in(&root, name, app_ron)
}

pub fn delete_profile(name: &str) -> Result<(), String> {
    let root = data_root().ok_or("could not find the settings folder")?;
    delete_profile_in(&root, name)
}

pub fn used_agent_ports() -> Vec<u16> {
    let Some(root) = data_root() else {
        return Vec::new();
    };
    used_agent_ports_in(&root)
}

/// Persisted app state from the Default profile, if that file can be read.
pub fn default_profile_app_ron() -> Option<String> {
    let root = data_root()?;
    let text = fs::read_to_string(root.join(directory_name(DEFAULT_SLUG)).join("app.ron")).ok()?;
    app_value_from_storage(&text)
}

pub fn next_agent_port(used: &[u16]) -> u16 {
    let mut port = DEFAULT_PORT;
    while used.contains(&port) && port < u16::MAX {
        port += 1;
    }
    port
}

pub fn can_relaunch() -> bool {
    std::env::current_exe().is_ok()
}

pub fn relaunch(profile: &str) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.env_remove("RUSTY_METER_PROFILE");
    if !profile.trim().eq_ignore_ascii_case(DEFAULT_NAME) {
        cmd.arg("--profile").arg(profile.trim());
    }
    cmd.spawn()?;
    Ok(())
}

pub fn open_new_window() -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.env_remove("RUSTY_METER_PROFILE");
    cmd.spawn()?;
    Ok(())
}

fn requested_name() -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--profile" {
            return args.next().map(|name| name.trim().to_owned());
        }
        if let Some(name) = arg.strip_prefix("--profile=") {
            return Some(name.trim().to_owned());
        }
    }
    std::env::var("RUSTY_METER_PROFILE")
        .ok()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
}

fn acquire_named(name: &str) -> Result<(), String> {
    if name.eq_ignore_ascii_case(DEFAULT_NAME) {
        return acquire_slug(DEFAULT_SLUG)
            .map_err(|_| format!("{DEFAULT_NAME} is open in another window."));
    }
    let slug = slug_for(name)?;
    acquire_slug(&slug).map_err(|_| format!("{name} is open in another window."))
}

fn acquire_slug(slug: &str) -> Result<(), String> {
    let root = data_root().ok_or("could not find the settings folder")?;
    let dir = root.join(directory_name(slug));
    fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("profile.lock"))
        .map_err(|err| err.to_string())?;
    file.try_lock()
        .map_err(|_| "profile is locked".to_owned())?;
    *HELD_LOCK.lock().unwrap() = Some(file);
    Ok(())
}

fn data_root() -> Option<PathBuf> {
    eframe::storage_dir("RustyMeter").and_then(|dir| dir.parent().map(Path::to_path_buf))
}

fn app_id_for_slug(slug: &str) -> String {
    if slug == DEFAULT_SLUG {
        "RustyMeter".to_owned()
    } else {
        format!("RustyMeter-{slug}")
    }
}

fn directory_name(slug: &str) -> String {
    app_id_for_slug(slug).to_ascii_lowercase()
}

fn slug_for(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Enter a profile name.".into());
    }
    if name.eq_ignore_ascii_case(DEFAULT_NAME) {
        return Ok(DEFAULT_SLUG.to_owned());
    }
    let mut slug = String::new();
    let mut pending_hyphen = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_hyphen && !slug.is_empty() {
                slug.push('-');
            }
            pending_hyphen = false;
            slug.push(ch.to_ascii_lowercase());
        } else {
            pending_hyphen = true;
        }
    }
    if slug.is_empty() {
        return Err("Use a name with letters or digits.".into());
    }
    if slug.starts_with("unsaved-") {
        return Err("That name is reserved.".into());
    }
    Ok(slug)
}

fn list_profiles_in(root: &Path) -> Vec<ListedProfile> {
    let mut profiles = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return profiles;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let Some(slug) = slug_from_directory(&entry.file_name().to_string_lossy()) else {
            continue;
        };
        let dir = entry.path();
        let name = fs::read_to_string(dir.join("name.txt"))
            .ok()
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| {
                if slug == DEFAULT_SLUG {
                    DEFAULT_NAME.to_owned()
                } else {
                    slug.clone()
                }
            });
        profiles.push(ListedProfile {
            locked: directory_is_locked(&dir),
            name,
        });
    }
    profiles.sort_by(|a, b| {
        let a_default = a.name.eq_ignore_ascii_case(DEFAULT_NAME);
        let b_default = b.name.eq_ignore_ascii_case(DEFAULT_NAME);
        b_default.cmp(&a_default).then_with(|| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
        })
    });
    profiles
}

fn slug_from_directory(dir_name: &str) -> Option<String> {
    if dir_name == "rustymeter" {
        return Some(DEFAULT_SLUG.to_owned());
    }
    let slug = dir_name.strip_prefix("rustymeter-")?;
    if slug.is_empty() || slug.starts_with("unsaved-") {
        return None;
    }
    Some(slug.to_owned())
}

fn directory_is_locked(dir: &Path) -> bool {
    let Ok(file) = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("profile.lock"))
    else {
        return false;
    };
    match file.try_lock() {
        Ok(()) => {
            let _ = file.unlock();
            false
        }
        Err(std::fs::TryLockError::WouldBlock) => true,
        Err(std::fs::TryLockError::Error(err)) => {
            log::debug!("profile lock probe failed: {err}");
            false
        }
    }
}

fn write_new_profile_in(root: &Path, name: &str, app_ron: &str) -> Result<(), String> {
    let slug = slug_for(name)?;
    if slug == DEFAULT_SLUG {
        return Err("Default already exists.".into());
    }
    let dir = root.join(directory_name(&slug));
    if dir.join("app.ron").exists() || dir.join("name.txt").exists() {
        return Err("A profile with that name already exists.".into());
    }
    fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    let mut kv = std::collections::HashMap::new();
    kv.insert("app".to_owned(), app_ron.to_owned());
    let mut file = File::create(dir.join("app.ron")).map_err(|err| err.to_string())?;
    ron::Options::default()
        .to_io_writer_pretty(&mut file, &kv, ron::ser::PrettyConfig::default())
        .map_err(|err| err.to_string())?;
    file.flush().map_err(|err| err.to_string())?;
    fs::write(dir.join("name.txt"), format!("{}\n", name.trim())).map_err(|err| err.to_string())?;
    Ok(())
}

fn delete_profile_in(root: &Path, name: &str) -> Result<(), String> {
    let slug = slug_for(name)?;
    if slug == DEFAULT_SLUG {
        return Err("Default stays. Other windows copy it when they open.".into());
    }
    let dir = root.join(directory_name(&slug));
    if !dir.is_dir() {
        return Err("That profile is already gone.".into());
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("profile.lock"))
        .map_err(|err| err.to_string())?;
    if lock.try_lock().is_err() {
        return Err(format!("{name} is open in another window."));
    }
    fs::remove_dir_all(&dir).map_err(|err| err.to_string())?;
    Ok(())
}

fn used_agent_ports_in(root: &Path) -> Vec<u16> {
    list_profiles_in(root)
        .into_iter()
        .filter_map(|profile| {
            let slug = slug_for(&profile.name).ok()?;
            let text = fs::read_to_string(root.join(directory_name(&slug)).join("app.ron")).ok()?;
            agent_port_in_text(&text)
        })
        .collect()
}

fn app_value_from_storage(text: &str) -> Option<String> {
    let kv: std::collections::HashMap<String, String> = ron::from_str(text).ok()?;
    kv.get("app").cloned()
}

fn agent_port_in_text(text: &str) -> Option<u16> {
    let rest = text.split("agent_port:").nth(1)?;
    let digits: String = rest
        .trim_start()
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_ignore_case_and_spaces() {
        assert_eq!(slug_for("Default").unwrap(), "default");
        assert_eq!(slug_for("Bench PSU").unwrap(), "bench-psu");
        assert_eq!(slug_for("bench psu").unwrap(), "bench-psu");
        assert!(slug_for("   ").is_err());
        assert!(slug_for("---").is_err());
    }

    #[test]
    fn directories_match_eframe_folder_names() {
        assert_eq!(directory_name("default"), "rustymeter");
        assert_eq!(directory_name("bench-psu"), "rustymeter-bench-psu");
    }

    #[test]
    fn reads_the_app_value_out_of_a_storage_file() {
        let text = "{\n    \"app\": \"serial_port:\\\"/dev/ttyUSB1\\\"\",\n}\n";
        assert_eq!(
            app_value_from_storage(text).as_deref(),
            Some("serial_port:\"/dev/ttyUSB1\"")
        );
    }

    #[test]
    fn next_port_skips_ports_already_stored() {
        assert_eq!(next_agent_port(&[]), DEFAULT_PORT);
        assert_eq!(next_agent_port(&[DEFAULT_PORT]), DEFAULT_PORT + 1);
        assert_eq!(
            next_agent_port(&[DEFAULT_PORT, DEFAULT_PORT + 1]),
            DEFAULT_PORT + 2
        );
    }

    #[test]
    fn save_as_roundtrip_lists_the_new_name_and_port() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("rustymeter")).unwrap();
        write_new_profile_in(root.path(), "Bench PSU", "agent_port:9741").unwrap();
        let listed = list_profiles_in(root.path());
        assert!(listed.iter().any(|profile| profile.name == "Bench PSU"));
        assert_eq!(used_agent_ports_in(root.path()), vec![9741]);
        let err = write_new_profile_in(root.path(), "bench psu", "agent_port:1").unwrap_err();
        assert!(err.contains("already exists"));
        delete_profile_in(root.path(), "Bench PSU").unwrap();
        assert!(
            !list_profiles_in(root.path())
                .iter()
                .any(|profile| profile.name == "Bench PSU")
        );
        let err = delete_profile_in(root.path(), "Default").unwrap_err();
        assert!(err.contains("Default stays"));
    }

    #[test]
    fn lock_is_visible_to_another_open_and_released_on_drop() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("rustymeter-bench-psu");
        fs::create_dir_all(&dir).unwrap();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("profile.lock"))
            .unwrap();
        file.try_lock().unwrap();
        assert!(directory_is_locked(&dir));
        let err = delete_profile_in(root.path(), "Bench PSU").unwrap_err();
        assert!(err.contains("open in another window"));
        drop(file);
        assert!(!directory_is_locked(&dir));
    }
}
