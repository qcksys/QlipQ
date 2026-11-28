//! Level and volume maths for the touch strip. Pure, so it is unit-tested.

/// Default bottom of the meter and fader scale; anything quieter draws as empty.
pub const FLOOR_DB: f32 = -60.0;
/// OBS's input volume range, limited to the mixer slider's 0 dB ceiling.
pub const VOLUME_MIN_DB: f32 = -100.0;
pub const VOLUME_MAX_DB: f32 = 0.0;
/// Default speed a displayed meter falls back after a peak, in percent of the bar per second.
pub const FALL_PERCENT_PER_S: f32 = 100.0;

pub fn mul_to_db(mul: f32) -> f32 {
    if mul > 0.0 {
        20.0 * mul.log10()
    } else {
        f32::NEG_INFINITY
    }
}

/// Maps dBFS onto 0..=100 across `floor_db..=0`.
pub fn db_to_percent(db: f32, floor_db: f32) -> f32 {
    if db.is_nan() || db <= floor_db {
        return 0.0;
    }
    ((db - floor_db) / -floor_db * 100.0).min(100.0)
}

/// The volume after turning the dial `ticks` steps of `step_db`.
pub fn step_volume(current_db: f32, ticks: i32, step_db: f32) -> f32 {
    if !current_db.is_finite() || current_db <= VOLUME_MIN_DB {
        // From silence, turning up starts at the bottom of the visible scale.
        if ticks <= 0 {
            return VOLUME_MIN_DB;
        }
        return (FLOOR_DB + ticks as f32 * step_db).clamp(VOLUME_MIN_DB, VOLUME_MAX_DB);
    }
    let next = current_db + ticks as f32 * step_db;
    ((next * 100.0).round() / 100.0).clamp(VOLUME_MIN_DB, VOLUME_MAX_DB)
}

/// Peak-hold falloff: jump up to a new peak at once, fall back at `fall_per_s` percent a second.
pub fn decay(shown: f32, target: f32, elapsed_s: f32, fall_per_s: f32) -> f32 {
    target.max(shown - fall_per_s * elapsed_s).max(0.0)
}

pub fn format_db(db: f32) -> String {
    if !db.is_finite() || db <= VOLUME_MIN_DB {
        "-inf dB".to_string()
    } else {
        format!("{db:.1} dB")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mul_to_db_handles_unity_half_and_silence() {
        assert_eq!(mul_to_db(1.0), 0.0);
        assert!((mul_to_db(0.5) - -6.0206).abs() < 0.001);
        assert_eq!(mul_to_db(0.0), f32::NEG_INFINITY);
    }

    #[test]
    fn db_to_percent_spans_floor_to_zero() {
        assert_eq!(db_to_percent(0.0, FLOOR_DB), 100.0);
        assert_eq!(db_to_percent(-30.0, FLOOR_DB), 50.0);
        assert_eq!(db_to_percent(-60.0, FLOOR_DB), 0.0);
        assert_eq!(db_to_percent(-90.0, FLOOR_DB), 0.0);
        assert_eq!(db_to_percent(f32::NEG_INFINITY, FLOOR_DB), 0.0);
        assert_eq!(db_to_percent(6.0, FLOOR_DB), 100.0);
    }

    #[test]
    fn db_to_percent_follows_the_floor() {
        assert_eq!(db_to_percent(-20.0, -40.0), 50.0);
        assert_eq!(db_to_percent(-45.0, -40.0), 0.0);
        assert_eq!(db_to_percent(-45.0, -90.0), 50.0);
    }

    #[test]
    fn step_volume_moves_by_step_and_clamps() {
        assert_eq!(step_volume(-10.0, 1, 0.2), -9.8);
        assert_eq!(step_volume(-10.0, -3, 0.2), -10.6);
        assert_eq!(step_volume(-0.1, 2, 0.2), 0.0);
        assert_eq!(step_volume(-99.9, -5, 0.2), -100.0);
    }

    #[test]
    fn step_volume_from_silence() {
        assert_eq!(step_volume(f32::NEG_INFINITY, 1, 0.2), -59.8);
        assert_eq!(step_volume(f32::NEG_INFINITY, -1, 0.2), -100.0);
        assert_eq!(step_volume(-100.0, 2, 0.2), -59.6);
    }

    #[test]
    fn decay_jumps_up_and_falls_slowly() {
        assert_eq!(decay(20.0, 80.0, 0.066, FALL_PERCENT_PER_S), 80.0);
        assert!((decay(80.0, 0.0, 0.1, FALL_PERCENT_PER_S) - 70.0).abs() < 0.001);
        assert!((decay(80.0, 0.0, 0.1, 400.0) - 40.0).abs() < 0.001);
        assert_eq!(decay(5.0, 0.0, 1.0, FALL_PERCENT_PER_S), 0.0);
    }

    #[test]
    fn format_db_shows_one_decimal_or_inf() {
        assert_eq!(format_db(-3.24), "-3.2 dB");
        assert_eq!(format_db(0.0), "0.0 dB");
        assert_eq!(format_db(-100.0), "-inf dB");
        assert_eq!(format_db(f32::NEG_INFINITY), "-inf dB");
    }
}
