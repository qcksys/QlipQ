//! The Set Output Device key: one press makes a device Windows' Default and Communications output,
//! and the key shows which of those two roles the device holds, one half of the key each.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Longer labels don't fit across the key at the smallest font size.
const MAX_LABEL_CHARS: usize = 18;
/// The deck's accent for output devices.
const ACCENT: &str = "#49d17d";

/// A Set Output Device key's settings, as its property inspector (`pi/output.html`) saves them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// The Windows (MMDevice) endpoint id, e.g. `{0.0.0.00000000}.{14300064-…}`.
    pub device_id: String,
    /// Text on the key; empty uses a short form of the device's name.
    pub label: String,
}

impl Settings {
    pub fn from_payload(payload: &Value) -> Self {
        let mut settings: Self =
            serde_json::from_value(payload["settings"].clone()).unwrap_or_default();
        settings.device_id = settings.device_id.trim().to_string();
        settings.label = settings
            .label
            .trim()
            .chars()
            .take(MAX_LABEL_CHARS)
            .collect();
        settings
    }
}

/// An output device, as Windows names it.
#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub id: String,
    pub name: String,
    /// Plugged in and enabled.
    pub active: bool,
}

/// Which devices hold the Default and Communications output roles.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Defaults {
    pub default: Option<String>,
    pub communications: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Look {
    /// No device chosen, or it is unplugged or disabled.
    Missing,
    Roles {
        default: bool,
        communications: bool,
    },
}

pub fn look(device_id: &str, device: Option<&Device>, defaults: &Defaults) -> Look {
    let holds = |role: &Option<String>| {
        role.as_deref()
            .is_some_and(|id| id.eq_ignore_ascii_case(device_id))
    };
    match device {
        Some(device) if device.active && !device_id.is_empty() => Look::Roles {
            default: holds(&defaults.default),
            communications: holds(&defaults.communications),
        },
        _ => Look::Missing,
    }
}

/// The key's text: the label setting, else the device name's last bracketed part
/// ("Speakers (Analogue 1/2) (Audient EVO16)" -> "Audient EVO16"), else the whole name.
pub fn label(settings: &Settings, device: Option<&Device>) -> String {
    if !settings.label.is_empty() {
        return settings.label.clone();
    }
    let Some(device) = device else {
        return "Set a device".into();
    };
    let short = device
        .name
        .rsplit_once('(')
        .and_then(|(_, rest)| rest.split_once(')'))
        .map(|(inside, _)| inside.trim())
        .filter(|inside| !inside.is_empty())
        .unwrap_or(&device.name);
    short.chars().take(MAX_LABEL_CHARS).collect()
}

/// Headphones get the headphones glyph; everything else is a speaker.
pub fn is_headphones(label: &str, device: Option<&Device>) -> bool {
    let mentions = |text: &str| text.to_ascii_lowercase().contains("headphone");
    mentions(label) || device.is_some_and(|device| mentions(&device.name))
}

/// The key image, as a data URL for `setImage`.
pub fn image(look: Look, label: &str, headphones: bool) -> String {
    format!(
        "data:image/svg+xml;base64,{}",
        BASE64.encode(key_svg(look, label, headphones))
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The deck's key style, split down the middle: the left half lights up in the accent gradient
/// while the device is the Default output, the right half while it's the Communications output.
fn key_svg(look: Look, label: &str, headphones: bool) -> String {
    const SPEAKER: &str = r#"<path d="M12 38 H30 L54 18 V82 L30 62 H12 Z"/><path d="M66 36 A20 20 0 0 1 66 64"/><path d="M77 24 A36 36 0 0 1 77 76"/>"#;
    const HEADPHONES: &str = r#"<path d="M16 66 V50 A34 34 0 0 1 84 50 V66"/><rect x="10" y="58" width="20" height="32" rx="7"/><rect x="70" y="58" width="20" height="32" rx="7"/>"#;
    let (default, communications) = match look {
        Look::Missing => (false, false),
        Look::Roles {
            default,
            communications,
        } => (default, communications),
    };
    let missing = look == Look::Missing;
    let half = |x: u32, on: bool| {
        if on {
            format!(r#"<rect x="{x}" width="72" height="144" fill="url(#on)"/>"#)
        } else {
            String::new()
        }
    };
    let bar = if missing { "#3d4858" } else { ACCENT };
    let glyph_color = match (missing, default || communications) {
        (true, _) => "#5c6878",
        (false, true) => "#ffffff",
        (false, false) => ACCENT,
    };
    let label_color = if missing { "#9fabbb" } else { "#f2f5f8" };
    let size = match label.chars().count() {
        0..=10 => 19,
        11..=13 => 17,
        _ => 15,
    };
    let caption = |x: u32, text: &str, on: bool| {
        let color = if on { "#ffffff" } else { "#6b7685" };
        format!(
            r#"<text x="{x}" y="132" text-anchor="middle" font-family="Segoe UI" font-weight="700" font-size="13" fill="{color}">{text}</text>"#
        )
    };
    let footer = if missing {
        r##"<text x="72" y="132" text-anchor="middle" font-family="Segoe UI" font-size="14" fill="#9fabbb">Not connected</text>"##.to_string()
    } else {
        format!(
            r##"<path d="M72 112 V140" stroke="#3d4858" stroke-width="1.5"/>{}{}"##,
            caption(36, "DEF", default),
            caption(108, "COM", communications)
        )
    };
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="144" height="144" viewBox="0 0 144 144"><defs><linearGradient id="off" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#202835"/><stop offset="1" stop-color="#0f141b"/></linearGradient><linearGradient id="on" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{ACCENT}" stop-opacity="0.55"/><stop offset="1" stop-color="{ACCENT}" stop-opacity="0.22"/></linearGradient></defs><rect width="144" height="144" fill="#0f141b"/><rect width="144" height="144" fill="url(#off)"/>{left}{right}<rect width="144" height="6" fill="{bar}"/><g transform="translate(47 14) scale(0.5)"><g fill="none" stroke="{glyph_color}" stroke-width="8" stroke-linecap="round" stroke-linejoin="round">{glyph}</g></g><text x="72" y="92" text-anchor="middle" font-family="Segoe UI" font-weight="600" font-size="{size}" fill="{label_color}">{label}</text>{footer}</svg>"##,
        left = half(0, default),
        right = half(72, communications),
        glyph = if headphones { HEADPHONES } else { SPEAKER },
        label = escape(label),
    )
}

#[cfg(windows)]
pub use win::Audio;

/// No audio devices off Windows; keys show "Not connected".
#[cfg(not(windows))]
pub struct Audio;

#[cfg(not(windows))]
impl Audio {
    pub fn new(_changed: tokio::sync::mpsc::UnboundedSender<()>) -> Option<Self> {
        None
    }
    pub fn devices(&self) -> Vec<Device> {
        Vec::new()
    }
    pub fn device(&self, _id: &str) -> Option<Device> {
        None
    }
    pub fn defaults(&self) -> Defaults {
        Defaults::default()
    }
    pub fn set_output(&self, _id: &str) -> anyhow::Result<()> {
        anyhow::bail!("audio devices are Windows-only")
    }
}

// COM method names follow Windows' own.
#[cfg(windows)]
#[allow(non_snake_case)]
mod win {
    use std::ffi::c_void;

    use anyhow::{Context, Result};
    use tokio::sync::mpsc::UnboundedSender;
    use windows::core::{GUID, HSTRING, PCWSTR};
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::Media::Audio::{
        eCommunications, eConsole, eMultimedia, eRender, EDataFlow, ERole, IMMDevice,
        IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Impl, MMDeviceEnumerator,
        DEVICE_STATE, DEVICE_STATE_ACTIVE,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED,
        STGM_READ,
    };
    use windows_core::{implement, interface, IUnknown, IUnknown_Vtbl, HRESULT};

    use super::{Defaults, Device};

    /// Windows' undocumented default-device switcher, as audio tools use it (Windows 7 and later).
    /// The method order must match the native vtable; only `SetDefaultEndpoint` is called.
    #[interface("f8679f50-850a-41cf-9c72-430f290290c8")]
    unsafe trait IPolicyConfig: IUnknown {
        fn GetMixFormat(&self, device: PCWSTR, format: *mut *mut c_void) -> HRESULT;
        fn GetDeviceFormat(
            &self,
            device: PCWSTR,
            default: i32,
            format: *mut *mut c_void,
        ) -> HRESULT;
        fn ResetDeviceFormat(&self, device: PCWSTR) -> HRESULT;
        fn SetDeviceFormat(
            &self,
            device: PCWSTR,
            endpoint: *mut c_void,
            mix: *mut c_void,
        ) -> HRESULT;
        fn GetProcessingPeriod(
            &self,
            device: PCWSTR,
            default: i32,
            period: *mut i64,
            minimum: *mut i64,
        ) -> HRESULT;
        fn SetProcessingPeriod(&self, device: PCWSTR, period: *mut i64) -> HRESULT;
        fn GetShareMode(&self, device: PCWSTR, mode: *mut c_void) -> HRESULT;
        fn SetShareMode(&self, device: PCWSTR, mode: *mut c_void) -> HRESULT;
        fn GetPropertyValue(
            &self,
            device: PCWSTR,
            key: *const PROPERTYKEY,
            value: *mut c_void,
        ) -> HRESULT;
        fn SetPropertyValue(
            &self,
            device: PCWSTR,
            key: *const PROPERTYKEY,
            value: *mut c_void,
        ) -> HRESULT;
        fn SetDefaultEndpoint(&self, device: PCWSTR, role: ERole) -> HRESULT;
        fn SetEndpointVisibility(&self, device: PCWSTR, visible: i32) -> HRESULT;
    }

    const POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

    /// Tells the plugin's main loop whenever a device or a default role changes.
    #[implement(IMMNotificationClient)]
    struct Notifier(UnboundedSender<()>);

    impl IMMNotificationClient_Impl for Notifier_Impl {
        fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> windows_core::Result<()> {
            let _ = self.0.send(());
            Ok(())
        }
        fn OnDeviceAdded(&self, _: &PCWSTR) -> windows_core::Result<()> {
            let _ = self.0.send(());
            Ok(())
        }
        fn OnDeviceRemoved(&self, _: &PCWSTR) -> windows_core::Result<()> {
            let _ = self.0.send(());
            Ok(())
        }
        fn OnDefaultDeviceChanged(
            &self,
            flow: EDataFlow,
            _: ERole,
            _: &PCWSTR,
        ) -> windows_core::Result<()> {
            if flow == eRender {
                let _ = self.0.send(());
            }
            Ok(())
        }
        fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &PROPERTYKEY) -> windows_core::Result<()> {
            Ok(())
        }
    }

    pub struct Audio {
        enumerator: IMMDeviceEnumerator,
        notifier: IMMNotificationClient,
    }

    impl Audio {
        /// Starts listening for device changes, which arrive as `()` on `changed`.
        pub fn new(changed: UnboundedSender<()>) -> Option<Self> {
            // SAFETY: plain COM setup on this thread; a second call just returns S_FALSE.
            let enumerator: IMMDeviceEnumerator = unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?
            };
            let notifier: IMMNotificationClient = Notifier(changed).into();
            // SAFETY: `notifier` stays alive in `Audio` until it is unregistered in `drop`.
            unsafe { enumerator.RegisterEndpointNotificationCallback(&notifier) }.ok()?;
            Some(Self {
                enumerator,
                notifier,
            })
        }

        /// Active output devices, for the settings picker.
        pub fn devices(&self) -> Vec<Device> {
            // SAFETY: COM calls on live interfaces.
            let Ok(collection) = (unsafe {
                self.enumerator
                    .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
            }) else {
                return Vec::new();
            };
            let count = unsafe { collection.GetCount() }.unwrap_or(0);
            let mut devices: Vec<Device> = (0..count)
                .filter_map(|index| describe(&unsafe { collection.Item(index) }.ok()?))
                .collect();
            devices.sort_by(|a, b| a.name.cmp(&b.name));
            devices
        }

        /// One device by id, whatever its state.
        pub fn device(&self, id: &str) -> Option<Device> {
            if id.is_empty() {
                return None;
            }
            // SAFETY: COM call on a live interface with a NUL-terminated id.
            let device = unsafe { self.enumerator.GetDevice(&HSTRING::from(id)) }.ok()?;
            describe(&device)
        }

        pub fn defaults(&self) -> Defaults {
            let id = |role| {
                // SAFETY: COM call on a live interface.
                let device =
                    unsafe { self.enumerator.GetDefaultAudioEndpoint(eRender, role) }.ok()?;
                device_id(&device)
            };
            Defaults {
                default: id(eConsole),
                communications: id(eCommunications),
            }
        }

        /// Makes `id` the Default output (console and multimedia) and the Communications output.
        pub fn set_output(&self, id: &str) -> Result<()> {
            // SAFETY: COM calls; `wide` is NUL-terminated and outlives them.
            unsafe {
                let policy: IPolicyConfig =
                    CoCreateInstance(&POLICY_CONFIG_CLIENT, None, CLSCTX_ALL)
                        .context("Windows' audio policy service isn't available")?;
                let wide = HSTRING::from(id);
                for role in [eConsole, eMultimedia, eCommunications] {
                    policy
                        .SetDefaultEndpoint(PCWSTR(wide.as_ptr()), role)
                        .ok()
                        .with_context(|| format!("setting the output device to {id}"))?;
                }
            }
            Ok(())
        }
    }

    impl Drop for Audio {
        fn drop(&mut self) {
            // SAFETY: unregisters the callback registered in `new`.
            let _ = unsafe {
                self.enumerator
                    .UnregisterEndpointNotificationCallback(&self.notifier)
            };
        }
    }

    fn device_id(device: &IMMDevice) -> Option<String> {
        // SAFETY: GetId returns a CoTaskMemAlloc'd string, which is copied and then freed.
        unsafe {
            let id = device.GetId().ok()?;
            let text = id.to_string().ok();
            CoTaskMemFree(Some(id.0 as *const c_void));
            text
        }
    }

    fn describe(device: &IMMDevice) -> Option<Device> {
        let id = device_id(device)?;
        // SAFETY: COM calls on a live device.
        let (name, state) = unsafe {
            let store = device.OpenPropertyStore(STGM_READ).ok()?;
            let name = store.GetValue(&PKEY_Device_FriendlyName).ok()?.to_string();
            (name, device.GetState().ok()?)
        };
        Some(Device {
            id,
            name,
            active: state == DEVICE_STATE_ACTIVE,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DX3: &str = "{0.0.0.00000000}.{14300064-89b3-4457-9a37-d15699cffab0}";
    const EVO: &str = "{0.0.0.00000000}.{dc729adc-319c-4711-b1c9-9af0aaa15357}";

    fn device(id: &str, name: &str, active: bool) -> Device {
        Device {
            id: id.into(),
            name: name.into(),
            active,
        }
    }

    #[test]
    fn settings_trim_and_cut_the_label() {
        let parsed = Settings::from_payload(&json!({ "settings": {
            "deviceId": format!(" {DX3} "), "label": " A label far too long for the key ",
        }}));
        assert_eq!(parsed.device_id, DX3);
        assert_eq!(parsed.label.chars().count(), MAX_LABEL_CHARS);
        assert_eq!(Settings::from_payload(&json!({})), Settings::default());
    }

    #[test]
    fn look_shows_each_role() {
        let dx3 = device(DX3, "Headphones (DX3 Pro+)", true);
        let both = Defaults {
            default: Some(DX3.into()),
            communications: Some(DX3.to_uppercase()),
        };
        assert_eq!(
            look(DX3, Some(&dx3), &both),
            Look::Roles {
                default: true,
                communications: true
            }
        );
        let split = Defaults {
            default: Some(EVO.into()),
            communications: Some(DX3.into()),
        };
        assert_eq!(
            look(DX3, Some(&dx3), &split),
            Look::Roles {
                default: false,
                communications: true
            }
        );
        assert_eq!(
            look(EVO, Some(&device(EVO, "Speakers (EVO16)", true)), &split),
            Look::Roles {
                default: true,
                communications: false
            }
        );
        assert_eq!(
            look(DX3, Some(&dx3), &Defaults::default()),
            Look::Roles {
                default: false,
                communications: false
            }
        );
    }

    #[test]
    fn look_is_missing_without_an_active_device() {
        let defaults = Defaults {
            default: Some(DX3.into()),
            communications: None,
        };
        assert_eq!(look(DX3, None, &defaults), Look::Missing);
        assert_eq!(
            look(DX3, Some(&device(DX3, "Headphones", false)), &defaults),
            Look::Missing
        );
        assert_eq!(look("", None, &defaults), Look::Missing);
    }

    #[test]
    fn label_falls_back_to_the_bracketed_name() {
        let settings = Settings::default();
        let evo = device(EVO, "Speakers (Analogue 1/2) (Audient EVO16)", true);
        assert_eq!(label(&settings, Some(&evo)), "Audient EVO16");
        assert_eq!(
            label(&settings, Some(&device(DX3, "Headphones (DX3 Pro+)", true))),
            "DX3 Pro+"
        );
        assert_eq!(
            label(&settings, Some(&device(EVO, "Realtek Audio", true))),
            "Realtek Audio"
        );
        assert_eq!(label(&settings, None), "Set a device");
        let named = Settings {
            label: "Desk".into(),
            ..Settings::default()
        };
        assert_eq!(label(&named, Some(&evo)), "Desk");
        assert!(is_headphones(
            "DX3",
            Some(&device(DX3, "Headphones (DX3 Pro+)", true))
        ));
        assert!(!is_headphones("EVO16", Some(&evo)));
    }

    fn decoded(image: &str) -> String {
        let svg = image
            .strip_prefix("data:image/svg+xml;base64,")
            .expect("an SVG data URL");
        String::from_utf8(BASE64.decode(svg).unwrap()).unwrap()
    }

    #[test]
    fn image_lights_each_half_for_its_role() {
        let left = r#"<rect x="0" width="72" height="144" fill="url(#on)"/>"#;
        let right = r#"<rect x="72" width="72" height="144" fill="url(#on)"/>"#;
        let draw = |default, communications| {
            decoded(&image(
                Look::Roles {
                    default,
                    communications,
                },
                "EVO16",
                false,
            ))
        };
        let both = draw(true, true);
        assert!(both.contains(left) && both.contains(right));
        let default_only = draw(true, false);
        assert!(default_only.contains(left) && !default_only.contains(right));
        let comms_only = draw(false, true);
        assert!(!comms_only.contains(left) && comms_only.contains(right));
        let neither = draw(false, false);
        assert!(!neither.contains(left) && !neither.contains(right));
        assert!(
            neither.contains(">DEF<") && neither.contains(">COM<") && neither.contains(">EVO16<")
        );
    }

    /// Reads this PC's output devices: `cargo test -p qlipq-streamdeck audio_reads -- --ignored --nocapture`.
    /// It only calls the switcher when one device already holds both roles, so nothing changes.
    #[cfg(windows)]
    #[test]
    #[ignore = "reads live audio devices"]
    fn audio_reads_this_pc() {
        let (changed, _events) = tokio::sync::mpsc::unbounded_channel();
        let audio = Audio::new(changed).expect("audio device enumerator");
        let devices = audio.devices();
        let defaults = audio.defaults();
        println!("{devices:#?}\n{defaults:#?}");
        assert!(!devices.is_empty());
        for device in &devices {
            assert_eq!(audio.device(&device.id).as_ref(), Some(device));
        }
        match (&defaults.default, &defaults.communications) {
            (Some(default), Some(communications)) if default == communications => {
                audio
                    .set_output(default)
                    .expect("re-applying the current output");
                assert_eq!(audio.defaults(), defaults);
                println!("re-applied {default} to all three roles");
            }
            _ => println!("Default and Communications differ; not calling the switcher"),
        }
    }

    #[test]
    fn image_for_a_missing_device_says_so() {
        let svg = decoded(&image(Look::Missing, "Wave & co", true));
        assert!(svg.contains(">Not connected<"));
        assert!(svg.contains(">Wave &amp; co<"));
        assert!(!svg.contains(">DEF<"));
    }
}
