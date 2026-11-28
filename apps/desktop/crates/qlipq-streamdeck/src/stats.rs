//! PC load for the stats dial: CPU, GPU and memory use, read from the counters Task Manager uses.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

const ROWS: usize = 3;
const DEFAULT_CPU_COLOR: &str = "#4fb3ff";
const DEFAULT_GPU_COLOR: &str = "#49d17d";
const DEFAULT_RAM_COLOR: &str = "#b48cff";

/// A PC Stats dial's settings, as its property inspector (`pi/stats.html`) saves them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// What each of the three rows shows, top to bottom: "cpu", "gpu", "ram" or "none".
    pub rows: Vec<String>,
    /// Seconds between updates.
    pub interval: f32,
    /// The GPU to show: "" for the busiest one, or an adapter id from `gpu_names`.
    pub gpu: String,
    pub cpu_color: String,
    pub gpu_color: String,
    pub ram_color: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            rows: vec!["cpu".into(), "gpu".into(), "ram".into()],
            interval: 1.0,
            gpu: String::new(),
            cpu_color: DEFAULT_CPU_COLOR.into(),
            gpu_color: DEFAULT_GPU_COLOR.into(),
            ram_color: DEFAULT_RAM_COLOR.into(),
        }
    }
}

fn is_hex_color(color: &str) -> bool {
    color.len() == 7 && color.starts_with('#') && color[1..].chars().all(|c| c.is_ascii_hexdigit())
}

impl Settings {
    pub fn from_payload(payload: &Value) -> Self {
        let mut settings: Self =
            serde_json::from_value(payload["settings"].clone()).unwrap_or_default();
        let defaults = Self::default();
        settings.rows.resize(ROWS, "none".into());
        for row in &mut settings.rows {
            if !matches!(row.as_str(), "cpu" | "gpu" | "ram") {
                *row = "none".into();
            }
        }
        if !(0.5..=5.0).contains(&settings.interval) {
            settings.interval = defaults.interval;
        }
        settings.gpu = settings.gpu.trim().to_ascii_lowercase();
        for (color, default) in [
            (&mut settings.cpu_color, defaults.cpu_color),
            (&mut settings.gpu_color, defaults.gpu_color),
            (&mut settings.ram_color, defaults.ram_color),
        ] {
            if !is_hex_color(color) {
                *color = default;
            }
        }
        settings
    }

    pub fn interval(&self) -> Duration {
        Duration::from_secs_f32(self.interval)
    }
}

/// One reading, as percentages; `None` (or a missing GPU) when Windows didn't provide it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sample {
    pub cpu: Option<f64>,
    /// Each GPU's figure, by adapter id (`luid_0x00000000_0x00012d26`).
    pub gpus: HashMap<String, f64>,
    pub memory: Option<f64>,
}

impl Sample {
    /// `adapter` "" means the busiest GPU.
    pub fn gpu(&self, adapter: &str) -> Option<f64> {
        if adapter.is_empty() {
            self.gpus.values().copied().reduce(f64::max)
        } else {
            self.gpus.get(adapter).copied()
        }
    }
}

/// Task Manager's per-GPU figure. Each engine (adapter LUID, physical GPU and engine index) is the
/// sum of every process's use of it, and a GPU's figure is its busiest engine. Takes
/// `GPU Engine(*)\Utilization Percentage` instance names and values; keys are lower-case adapter ids.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn gpu_by_adapter<'a>(
    readings: impl IntoIterator<Item = (&'a str, f64)>,
) -> HashMap<String, f64> {
    let mut engines: HashMap<&str, f64> = HashMap::new();
    for (instance, percent) in readings {
        *engines.entry(engine_of(instance)).or_default() += percent;
    }
    let mut gpus: HashMap<String, f64> = HashMap::new();
    for (engine, percent) in engines {
        let adapter = engine
            .split_once("_phys")
            .map_or(engine, |(adapter, _)| adapter)
            .to_ascii_lowercase();
        let busiest = gpus.entry(adapter).or_default();
        *busiest = busiest.max(percent.min(100.0));
    }
    gpus
}

/// `pid_12_luid_0x0_0x12D26_phys_0_eng_3_engtype_3D` -> `luid_0x0_0x12D26_phys_0_eng_3`.
#[cfg_attr(not(windows), allow(dead_code))]
fn engine_of(instance: &str) -> &str {
    let start = instance.find("luid_").unwrap_or(0);
    let end = instance.find("_engtype").unwrap_or(instance.len());
    instance.get(start..end).unwrap_or(instance)
}

/// The `setFeedback` payload for the stats layout (`layouts/stats.json`), whose rows are
/// `row1`..`row3` with a label, bar and value each.
pub fn feedback(sample: &Sample, settings: &Settings) -> Value {
    let text = |percent: Option<f64>| percent.map_or("–".to_string(), |p| format!("{p:.0}%"));
    let bar = |percent: Option<f64>| percent.map_or(0, |p| p.round().clamp(0.0, 100.0) as i32);
    let mut payload = Map::new();
    for (index, row) in settings.rows.iter().enumerate() {
        let n = index + 1;
        let shown = match row.as_str() {
            "cpu" => Some(("CPU", sample.cpu, &settings.cpu_color)),
            "gpu" => Some(("GPU", sample.gpu(&settings.gpu), &settings.gpu_color)),
            "ram" => Some(("RAM", sample.memory, &settings.ram_color)),
            _ => None,
        };
        let enabled = shown.is_some();
        let (label, percent, color) = shown.unwrap_or(("", None, &settings.cpu_color));
        payload.insert(
            format!("row{n}Label"),
            json!({ "value": label, "enabled": enabled }),
        );
        payload.insert(
            format!("row{n}Bar"),
            json!({ "value": bar(percent), "bar_fill_c": color, "enabled": enabled }),
        );
        payload.insert(
            format!("row{n}Value"),
            json!({ "value": text(percent), "enabled": enabled }),
        );
    }
    Value::Object(payload)
}

/// GPU names for the settings picker, as `(adapter id, name)`, from the adapter list DirectX keeps
/// in the registry. Its ids match the GPU counters' (checked against `GPU Engine` instances).
#[cfg(windows)]
pub fn gpu_names() -> Vec<(String, String)> {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;

    let Ok(directx) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(r"SOFTWARE\Microsoft\DirectX")
    else {
        return Vec::new();
    };
    let mut names: Vec<(String, String)> = directx
        .enum_keys()
        .filter_map(Result::ok)
        .filter_map(|name| {
            let adapter = directx.open_subkey(name).ok()?;
            let description: String = adapter.get_value("Description").ok()?;
            let luid: u64 = adapter.get_value("AdapterLuid").ok()?;
            Some((adapter_id(luid), description))
        })
        .filter(|(_, name)| name != "Microsoft Basic Render Driver")
        .collect();
    names.sort();
    names.dedup();
    names
}

#[cfg(not(windows))]
pub fn gpu_names() -> Vec<(String, String)> {
    Vec::new()
}

/// The counters' id for an adapter LUID (high part, then low part).
#[cfg_attr(not(windows), allow(dead_code))]
fn adapter_id(luid: u64) -> String {
    format!("luid_0x{:08x}_0x{:08x}", luid >> 32, luid & 0xffff_ffff)
}

#[cfg(windows)]
pub use pdh::Sampler;

/// No counters off Windows; the dial shows dashes.
#[cfg(not(windows))]
pub struct Sampler;

#[cfg(not(windows))]
impl Sampler {
    pub fn new() -> Option<Self> {
        None
    }

    pub fn sample(&mut self) -> Sample {
        Sample::default()
    }
}

#[cfg(windows)]
mod pdh {
    use std::collections::HashMap;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::System::Performance::{
        PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
        PdhGetFormattedCounterValue, PdhOpenQueryW, PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA,
        PDH_FMT_COUNTERVALUE, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER,
        PDH_HQUERY, PDH_MORE_DATA,
    };
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    use super::{gpu_by_adapter, Sample};

    /// Task Manager's CPU figure (it accounts for boost clocks, unlike `% Processor Time`).
    const CPU_COUNTER: &str = r"\Processor Information(_Total)\% Processor Utility";
    const GPU_COUNTER: &str = r"\GPU Engine(*)\Utilization Percentage";

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    fn valid(value: &PDH_FMT_COUNTERVALUE) -> bool {
        matches!(value.CStatus, PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA)
    }

    pub struct Sampler {
        query: PDH_HQUERY,
        cpu: PDH_HCOUNTER,
        gpu: PDH_HCOUNTER,
    }

    impl Sampler {
        /// Opens the counters. Usage is a rate between two collections, so the first `sample`
        /// covers the time since this call.
        pub fn new() -> Option<Self> {
            let mut query = null_mut();
            // SAFETY: `query` is a valid out pointer; a null data source means live data.
            if unsafe { PdhOpenQueryW(null(), 0, &mut query) } != 0 {
                return None;
            }
            let mut sampler = Self {
                query,
                cpu: null_mut(),
                gpu: null_mut(),
            };
            sampler.cpu = sampler.add(CPU_COUNTER);
            sampler.gpu = sampler.add(GPU_COUNTER);
            // SAFETY: `query` is open.
            unsafe { PdhCollectQueryData(sampler.query) };
            Some(sampler)
        }

        /// A counter handle, or null if this PC doesn't have the counter.
        fn add(&self, path: &str) -> PDH_HCOUNTER {
            let path = wide(path);
            let mut counter = null_mut();
            // SAFETY: `path` is NUL-terminated and outlives the call; `counter` is a valid out pointer.
            let status =
                unsafe { PdhAddEnglishCounterW(self.query, path.as_ptr(), 0, &mut counter) };
            if status == 0 {
                counter
            } else {
                null_mut()
            }
        }

        pub fn sample(&mut self) -> Sample {
            // SAFETY: `query` is open.
            let collected = unsafe { PdhCollectQueryData(self.query) } == 0;
            Sample {
                cpu: if collected { self.cpu() } else { None },
                gpus: if collected {
                    self.gpus()
                } else {
                    Default::default()
                },
                memory: memory(),
            }
        }

        fn cpu(&self) -> Option<f64> {
            if self.cpu.is_null() {
                return None;
            }
            let mut value = PDH_FMT_COUNTERVALUE::default();
            // SAFETY: `cpu` is a counter in the open query; `value` is a valid out pointer.
            let status = unsafe {
                PdhGetFormattedCounterValue(self.cpu, PDH_FMT_DOUBLE, null_mut(), &mut value)
            };
            // SAFETY: PDH_FMT_DOUBLE fills `doubleValue`, and the zeroed default is a valid f64.
            (status == 0 && valid(&value)).then_some(unsafe { value.Anonymous.doubleValue })
        }

        fn gpus(&self) -> HashMap<String, f64> {
            if self.gpu.is_null() {
                return HashMap::new();
            }
            let (mut size, mut count) = (0u32, 0u32);
            // SAFETY: a null buffer asks for the size needed.
            let status = unsafe {
                PdhGetFormattedCounterArrayW(
                    self.gpu,
                    PDH_FMT_DOUBLE,
                    &mut size,
                    &mut count,
                    null_mut(),
                )
            };
            if status != PDH_MORE_DATA {
                return HashMap::new();
            }
            // The buffer holds the items followed by their names; u64s keep the items aligned.
            let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
            let items_ptr = buffer.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
            // SAFETY: `buffer` is at least `size` bytes and aligned for the items.
            let status = unsafe {
                PdhGetFormattedCounterArrayW(
                    self.gpu,
                    PDH_FMT_DOUBLE,
                    &mut size,
                    &mut count,
                    items_ptr,
                )
            };
            if status != 0 {
                return HashMap::new();
            }
            // SAFETY: PDH wrote `count` items at the start of `buffer`, which is still alive.
            let items = unsafe { std::slice::from_raw_parts(items_ptr, count as usize) };
            let readings: Vec<(String, f64)> = items
                .iter()
                .filter(|item| valid(&item.FmtValue))
                .map(|item| {
                    // SAFETY: each name is a NUL-terminated string inside `buffer`; PDH_FMT_DOUBLE
                    // fills `doubleValue`.
                    unsafe { (name(item.szName), item.FmtValue.Anonymous.doubleValue) }
                })
                .collect();
            gpu_by_adapter(
                readings
                    .iter()
                    .map(|(name, percent)| (name.as_str(), *percent)),
            )
        }
    }

    /// # Safety
    /// `ptr` must point to a NUL-terminated UTF-16 string.
    unsafe fn name(ptr: *const u16) -> String {
        let mut len = 0;
        // SAFETY: the caller guarantees a terminator, so every read before it is in bounds.
        while unsafe { *ptr.add(len) } != 0 {
            len += 1;
        }
        // SAFETY: the `len` units before the terminator are initialised.
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(ptr, len) })
    }

    impl Drop for Sampler {
        fn drop(&mut self) {
            // SAFETY: `query` is open and closed only here.
            unsafe { PdhCloseQuery(self.query) };
        }
    }

    /// In-use memory as a share of installed RAM, as Task Manager shows it.
    fn memory() -> Option<f64> {
        let mut status = MEMORYSTATUSEX {
            dwLength: size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        // SAFETY: `status` is a valid MEMORYSTATUSEX with `dwLength` set.
        if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 || status.ullTotalPhys == 0 {
            return None;
        }
        let used = status.ullTotalPhys.saturating_sub(status.ullAvailPhys);
        Some(used as f64 * 100.0 / status.ullTotalPhys as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RTX: &str = "luid_0x00000000_0x00012d26";
    const IGPU: &str = "luid_0x00000000_0x00017aa1";

    fn two_gpus() -> HashMap<String, f64> {
        gpu_by_adapter([
            (
                "pid_1_luid_0x00000000_0x00012D26_phys_0_eng_0_engtype_3D",
                40.0,
            ),
            (
                "pid_2_luid_0x00000000_0x00012D26_phys_0_eng_0_engtype_3D",
                37.0,
            ),
            (
                "pid_1_luid_0x00000000_0x00012D26_phys_0_eng_7_engtype_VideoEncode",
                19.4,
            ),
            // The integrated GPU is a different adapter, so it doesn't add to the RTX's engine.
            (
                "pid_3_luid_0x00000000_0x00017AA1_phys_0_eng_0_engtype_3D",
                60.0,
            ),
        ])
    }

    #[test]
    fn gpu_by_adapter_takes_each_gpus_busiest_engine() {
        let gpus = two_gpus();
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[RTX], 77.0);
        assert_eq!(gpus[IGPU], 60.0);
    }

    #[test]
    fn gpu_by_adapter_caps_at_100_and_handles_no_readings() {
        let gpus = gpu_by_adapter([
            ("pid_1_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 70.0),
            ("pid_2_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 50.0),
        ]);
        assert_eq!(gpus["luid_0x0_0x1"], 100.0);
        assert!(gpu_by_adapter([]).is_empty());
    }

    #[test]
    fn sample_gpu_is_the_busiest_or_the_chosen_one() {
        let sample = Sample {
            gpus: two_gpus(),
            ..Sample::default()
        };
        assert_eq!(sample.gpu(""), Some(77.0));
        assert_eq!(sample.gpu(IGPU), Some(60.0));
        assert_eq!(sample.gpu("luid_0x00000000_0x0000dead"), None);
        assert_eq!(Sample::default().gpu(""), None);
    }

    #[test]
    fn adapter_id_matches_the_counter_format() {
        assert_eq!(adapter_id(0x12D26), RTX);
        assert_eq!(adapter_id((1 << 32) | 0xAB), "luid_0x00000001_0x000000ab");
    }

    #[test]
    fn settings_default_and_sanitise() {
        assert_eq!(Settings::from_payload(&json!({})), Settings::default());
        let parsed = Settings::from_payload(&json!({ "settings": {
            "rows": ["ram", "disk"], "interval": 60, "gpu": " LUID_0x00000000_0x00017AA1 ",
            "cpuColor": "red", "gpuColor": "#12ab3C",
        }}));
        assert_eq!(parsed.rows, ["ram", "none", "none"]);
        assert_eq!(parsed.interval, 1.0);
        assert_eq!(parsed.gpu, IGPU);
        assert_eq!(parsed.cpu_color, DEFAULT_CPU_COLOR);
        assert_eq!(parsed.gpu_color, "#12ab3C");
        let parsed = Settings::from_payload(&json!({ "settings": { "interval": 2.5 } }));
        assert_eq!(parsed.interval(), Duration::from_millis(2500));
    }

    #[test]
    fn engine_of_falls_back_to_the_whole_name() {
        assert_eq!(engine_of("unexpected"), "unexpected");
        assert_eq!(
            engine_of("pid_9_luid_0x0_0x1_phys_0_eng_2_engtype_Copy"),
            "luid_0x0_0x1_phys_0_eng_2"
        );
    }

    /// Reads this PC's real counters: `cargo test -p qlipq-streamdeck sampler_reads -- --ignored --nocapture`.
    #[cfg(windows)]
    #[test]
    #[ignore = "reads live system counters"]
    fn sampler_reads_this_pc() {
        let mut sampler = Sampler::new().expect("PDH query");
        std::thread::sleep(std::time::Duration::from_secs(1));
        let started = std::time::Instant::now();
        let sample = sampler.sample();
        println!("{sample:?} in {:?}", started.elapsed());
        println!("GPU names: {:?}", gpu_names());
        for percent in [sample.cpu, sample.gpu(""), sample.memory] {
            assert!(
                matches!(percent, Some(p) if (0.0..=100.0).contains(&p)),
                "{sample:?}"
            );
        }
        // Every GPU offered in the settings must have a reading (the counters also report
        // Microsoft's software renderer, which isn't offered).
        let names = gpu_names();
        assert!(!names.is_empty());
        for (id, name) in &names {
            assert!(
                sample.gpus.contains_key(id),
                "no reading for {name} ({id}): {:?}",
                sample.gpus
            );
        }
    }

    #[test]
    fn feedback_fills_rows_in_the_chosen_order() {
        let sample = Sample {
            cpu: Some(37.4),
            gpus: HashMap::new(),
            memory: Some(100.6),
        };
        let settings = Settings {
            rows: vec!["ram".into(), "gpu".into(), "cpu".into()],
            ..Settings::default()
        };
        let payload = feedback(&sample, &settings);
        assert_eq!(payload["row1Label"]["value"], "RAM");
        assert_eq!(payload["row1Bar"]["value"], 100);
        assert_eq!(payload["row1Bar"]["bar_fill_c"], DEFAULT_RAM_COLOR);
        assert_eq!(payload["row2Value"]["value"], "–", "no GPU reading");
        assert_eq!(payload["row2Bar"]["value"], 0);
        assert_eq!(payload["row3Value"]["value"], "37%");
        assert_eq!(payload["row3Bar"]["value"], 37);
    }

    #[test]
    fn feedback_hides_unused_rows_and_uses_the_chosen_gpu() {
        let sample = Sample {
            gpus: two_gpus(),
            ..Sample::default()
        };
        let settings = Settings {
            rows: vec!["gpu".into(), "none".into(), "none".into()],
            gpu: IGPU.into(),
            gpu_color: "#ff0000".into(),
            ..Settings::default()
        };
        let payload = feedback(&sample, &settings);
        assert_eq!(payload["row1Value"]["value"], "60%");
        assert_eq!(payload["row1Bar"]["bar_fill_c"], "#ff0000");
        assert_eq!(payload["row1Bar"]["enabled"], true);
        for item in ["row2Label", "row2Bar", "row2Value", "row3Bar"] {
            assert_eq!(payload[item]["enabled"], false, "{item}");
        }
    }
}
