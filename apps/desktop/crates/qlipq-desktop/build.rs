//! Ship the shared FFmpeg runtime DLLs next to the built `qlipq` exe so it can find
//! avcodec/avformat/avfilter/etc. at runtime (otherwise: STATUS_DLL_NOT_FOUND / 0xc0000135).
//! No-op unless `FFMPEG_DLL_DIR` is set (so builds without the shared FFmpeg dev build are unaffected).

use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=FFMPEG_DLL_DIR");

    let Ok(bin) = env::var("FFMPEG_DLL_DIR") else {
        return;
    };
    let Ok(out) = env::var("OUT_DIR") else {
        return;
    };
    // OUT_DIR = target/<profile>/build/<pkg>-<hash>/out → up 3 = target/<profile> (next to the exe).
    let Some(target_dir) = PathBuf::from(out).ancestors().nth(3).map(PathBuf::from) else {
        return;
    };
    let Ok(entries) = fs::read_dir(&bin) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) == Some("dll") {
            if let Some(name) = p.file_name() {
                let _ = fs::copy(&p, target_dir.join(name));
            }
        }
    }
}
