use std::ffi::OsString;
use std::io;
use std::path::Path;

use winreg::enums::HKEY_CURRENT_USER;
#[cfg(not(test))]
use winreg::enums::KEY_SET_VALUE;
use winreg::RegKey;

#[cfg(not(test))]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "QlipQ";

pub fn set_enabled(enabled: bool) -> io::Result<()> {
    crate::background::assert_worker();
    #[cfg(test)]
    {
        TEST_ENABLED.store(enabled, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
    #[cfg(not(test))]
    {
    let executable = std::env::current_exe()?;
    let (key, _) =
        RegKey::predef(HKEY_CURRENT_USER).create_subkey_with_flags(RUN_KEY, KEY_SET_VALUE)?;
    update_run_key(&key, enabled, &executable)
    }
}

#[cfg(test)]
pub static TEST_ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn update_run_key(key: &RegKey, enabled: bool, executable: &Path) -> io::Result<()> {
    if enabled {
        let mut command = OsString::from("\"");
        command.push(executable);
        command.push("\"");
        key.set_value(VALUE_NAME, &command)
    } else {
        match key.delete_value(VALUE_NAME) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn registration_quotes_paths_updates_and_removes_only_qlipq() {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let test_path = format!(
            r"Software\QlipQ-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let (key, _) = hkcu.create_subkey(&test_path).unwrap();
        let result = std::panic::catch_unwind(|| {
            key.set_value("OtherApp", &"unchanged").unwrap();
            let executable = Path::new(r"C:\Program Files\QlipQ 日本語\qlipq.exe");

            update_run_key(&key, false, executable).unwrap();
            update_run_key(&key, true, executable).unwrap();
            assert_eq!(
                key.get_value::<String, _>(VALUE_NAME).unwrap(),
                "\"C:\\Program Files\\QlipQ 日本語\\qlipq.exe\""
            );

            update_run_key(&key, true, Path::new(r"D:\QlipQ\qlipq.exe")).unwrap();
            assert_eq!(
                key.get_value::<String, _>(VALUE_NAME).unwrap(),
                "\"D:\\QlipQ\\qlipq.exe\""
            );

            update_run_key(&key, false, executable).unwrap();
            update_run_key(&key, false, executable).unwrap();
            assert_eq!(
                key.get_value::<String, _>(VALUE_NAME).unwrap_err().kind(),
                io::ErrorKind::NotFound
            );
            assert_eq!(key.get_value::<String, _>("OtherApp").unwrap(), "unchanged");
        });
        drop(key);
        hkcu.delete_subkey(&test_path).unwrap();
        if let Err(error) = result {
            std::panic::resume_unwind(error);
        }
    }
}
