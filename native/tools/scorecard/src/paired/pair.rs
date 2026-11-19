//! One paired slot heard three ways, the reference mic, the phone with the stage off and as it
//! ran, as the scorecard's numbers and as a table for whoever is tuning the stage.

use std::collections::BTreeMap;

use super::gap::{BANDS_READ, Gap};
use super::heard::Heard;

pub struct Pair {
    pub mic: Heard,
    pub off: Heard,
    /// None when the take never turned the stage on.
    pub on: Option<Heard>,
}

impl Pair {
    /// Each row by its name in the scorecard, as `slot0.pairDistanceDb.off`.
    pub fn numbers(&self, slot: usize) -> BTreeMap<String, f64> {
        let mut numbers = BTreeMap::new();
        let mut put = |row: &str, take: &str, value: f64| {
            numbers.insert(format!("slot{slot}.{row}.{take}"), (value * 100.0).round() / 100.0);
        };
        for (take, heard) in self.takes() {
            if take != "reference" {
                let gap = Gap::between(heard, &self.mic);
                put("pairDistanceDb", take, gap.distance_db);
                for ((row, _, _), value) in BANDS_READ.iter().zip(gap.bands_db) {
                    put(row, take, value);
                }
            }
            if let Some(sibilance) = heard.sibilance_db {
                put("sibilanceDb", take, sibilance);
            }
            put("plosivesPerMinute", take, heard.plosives_per_minute);
            put("noiseFloorDb", take, heard.noise_floor_db);
        }
        numbers
    }

    /// The rows as a table, mic, stage off and stage on, closed by what the stage did, in words.
    pub fn table(&self) -> Vec<String> {
        let heard = [Some(&self.mic), Some(&self.off), self.on.as_ref()];
        let gaps = [None, Some(&self.off), self.on.as_ref()]
            .map(|phone| phone.map(|phone| Gap::between(phone, &self.mic)));
        let gap = |pick: &dyn Fn(&Gap) -> f64| gaps.each_ref().map(|gap| gap.as_ref().map(pick));
        let take = |pick: &dyn Fn(&Heard) -> Option<f64>| heard.map(|take| take.and_then(pick));
        let mut rows = vec![("distance from the mic, dB".to_owned(), gap(&|g| g.distance_db))];
        for (band, (_, words, _)) in BANDS_READ.iter().enumerate() {
            rows.push((format!("{words}, dB"), gap(&|g| g.bands_db[band])));
        }
        rows.push(("s over 5 kHz against singing, dB".into(), take(&|h| h.sibilance_db)));
        rows.push(("pops under 100 Hz a minute".into(), take(&|h| Some(h.plosives_per_minute))));
        rows.push(("room against singing, dB".into(), take(&|h| Some(h.noise_floor_db))));
        let cell =
            |value: Option<f64>| value.map_or(format!("{:>11}", "-"), |v| format!("{v:>11.2}"));
        let mut lines =
            vec![format!("  {:<34}{:>11}{:>11}{:>11}", "", "mic", "stage off", "stage on")];
        lines.extend(
            rows.into_iter()
                .map(|(label, cells)| format!("  {label:<34}{}", cells.map(cell).concat())),
        );
        let off = gaps[1].as_ref().map_or(0.0, |gap| gap.distance_db);
        let sounds = format!("  With the stage off this phone sounds {off:.2} dB from the mic");
        lines.push(match &gaps[2] {
            Some(on) if on.distance_db <= off => {
                format!("{sounds}, and the stage brings it {:.2} dB closer.", off - on.distance_db)
            }
            Some(on) => {
                format!(
                    "{sounds}, and the stage takes it {:.2} dB further away.",
                    on.distance_db - off
                )
            }
            None => format!(
                "{sounds}. The stage was never on in this take, so it has no stage-on column."
            ),
        });
        lines.push(
            "  Distance is how far the phone's tone sits from the mic's at the same loudness, the RMS \
             over third octaves from 100 Hz to 12.5 kHz while singing; 0 is the mic."
                .into(),
        );
        lines
    }

    fn takes(&self) -> impl Iterator<Item = (&'static str, &Heard)> {
        [("reference", Some(&self.mic)), ("off", Some(&self.off)), ("on", self.on.as_ref())]
            .into_iter()
            .filter_map(|(take, heard)| Some((take, heard?)))
    }
}
