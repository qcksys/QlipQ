use std::cmp::Ordering;

use qlipq_core::queue::QueueItem;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum QueueSort {
    #[default]
    Newest,
    Oldest,
    NameAsc,
    NameDesc,
    Shortest,
    Longest,
    Largest,
    Smallest,
}

impl QueueSort {
    pub const ALL: &'static [Self] = &[
        Self::Newest,
        Self::Oldest,
        Self::NameAsc,
        Self::NameDesc,
        Self::Shortest,
        Self::Longest,
        Self::Largest,
        Self::Smallest,
    ];

    pub fn sort(self, items: &mut Vec<&QueueItem>) {
        match self {
            Self::Newest | Self::Oldest => items.sort_by_cached_key(|item| {
                let date = [&item.recorded_at, &item.file_modified_at]
                    .into_iter()
                    .filter_map(|date| date.as_deref())
                    .chain(std::iter::once(item.added_at.as_str()))
                    .find_map(|date| chrono::DateTime::parse_from_rfc3339(date).ok());
                date.map(|date| date.timestamp_millis())
            }),
            Self::NameAsc | Self::NameDesc => {
                items.sort_by_cached_key(|item| item.file_name.to_lowercase())
            }
            Self::Shortest | Self::Longest => items.sort_by(|a, b| {
                compare_optional(
                    a.duration_sec.filter(|d| d.is_finite() && *d >= 0.0),
                    b.duration_sec.filter(|d| d.is_finite() && *d >= 0.0),
                    self == Self::Longest,
                )
            }),
            Self::Largest | Self::Smallest => items.sort_by(|a, b| {
                compare_optional(a.file_size_bytes, b.file_size_bytes, self == Self::Largest)
            }),
        }
        if matches!(self, Self::Newest | Self::NameDesc) {
            items.reverse();
        }
    }
}

fn compare_optional<T: PartialOrd>(a: Option<T>, b: Option<T>, descending: bool) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => {
            let order = a.partial_cmp(&b).unwrap_or(Ordering::Equal);
            if descending {
                order.reverse()
            } else {
                order
            }
        }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

impl std::fmt::Display for QueueSort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Newest => "Newest first",
            Self::Oldest => "Oldest first",
            Self::NameAsc => "Name A–Z",
            Self::NameDesc => "Name Z–A",
            Self::Shortest => "Shortest first",
            Self::Longest => "Longest first",
            Self::Largest => "Largest first",
            Self::Smallest => "Smallest first",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn item(
        name: &str,
        date: Option<&str>,
        modified: &str,
        duration: Option<f64>,
        size: Option<i64>,
    ) -> QueueItem {
        serde_json::from_value(json!({
            "id": name, "path": name, "fileName": name, "status": "ready",
            "addedAt": "2026-10-04T00:00:00Z", "recordedAt": date,
            "fileModifiedAt": modified, "durationSec": duration, "fileSizeBytes": size,
        }))
        .unwrap()
    }

    #[test]
    fn sort_choices_use_metadata_and_put_unknown_numbers_last() {
        let items = [
            item(
                "alpha.mp4",
                Some("2026-10-01T10:00:00+10:00"),
                "2026-10-04T00:00:00Z",
                Some(30.0),
                Some(200),
            ),
            item(
                "Bravo.mp4",
                None,
                "2026-10-02T00:00:00Z",
                Some(10.0),
                Some(100),
            ),
            item(
                "charlie.mp4",
                Some("2026-10-01T01:00:00Z"),
                "2026-10-04T00:00:00Z",
                None,
                None,
            ),
        ];
        for (sort, expected) in [
            (QueueSort::Newest, [1, 2, 0]),
            (QueueSort::Oldest, [0, 2, 1]),
            (QueueSort::NameAsc, [0, 1, 2]),
            (QueueSort::NameDesc, [2, 1, 0]),
            (QueueSort::Shortest, [1, 0, 2]),
            (QueueSort::Longest, [0, 1, 2]),
            (QueueSort::Largest, [0, 1, 2]),
            (QueueSort::Smallest, [1, 0, 2]),
        ] {
            let mut sorted = items.iter().collect();
            sort.sort(&mut sorted);
            assert_eq!(sorted, expected.map(|i| &items[i]), "{sort}");
        }
    }
}
