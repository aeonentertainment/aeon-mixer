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
    /// Each row a scorecard number per take, as `slot0.pairDistanceDb.off`.
    pub fn numbers(&self, slot: usize) -> BTreeMap<String, f64> {
        let mut numbers = BTreeMap::new();
        for (name, _, cells) in self.rows() {
            for (take, cell) in ["reference", "off", "on"].into_iter().zip(cells) {
                if let Some(value) = cell {
                    numbers.insert(
                        format!("slot{slot}.{name}.{take}"),
                        (value * 100.0).round() / 100.0,
                    );
                }
            }
        }
        numbers
    }

    /// The rows as a table, mic, stage off and stage on, closed by what the stage did, in words.
    pub fn table(&self) -> Vec<String> {
        let rows = self.rows();
        let (off, on) = (rows[0].2[1].unwrap_or_default(), rows[0].2[2]);
        let cell =
            |value: Option<f64>| value.map_or(format!("{:>11}", "-"), |v| format!("{v:>11.2}"));
        let mut lines =
            vec![format!("  {:<34}{:>11}{:>11}{:>11}", "", "mic", "stage off", "stage on")];
        lines.extend(
            rows.into_iter()
                .map(|(_, words, cells)| format!("  {words:<34}{}", cells.map(cell).concat())),
        );
        let sounds = format!("  With the stage off this phone sounds {off:.2} dB from the mic");
        lines.push(match on {
            Some(on) if on <= off => {
                format!("{sounds}, and the stage brings it {:.2} dB closer.", off - on)
            }
            Some(on) => {
                format!("{sounds}, and the stage takes it {:.2} dB further away.", on - off)
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

    /// Each row: its number's name, its words in the table, and its value for the mic, the stage
    /// off and the stage on, where it has one. The gaps have none for the mic, nor a stage that
    /// was never on.
    fn rows(&self) -> Vec<(&'static str, String, [Option<f64>; 3])> {
        let heard = [Some(&self.mic), Some(&self.off), self.on.as_ref()];
        let gaps = [None, Some(&self.off), self.on.as_ref()]
            .map(|phone| phone.map(|phone| Gap::between(phone, &self.mic)));
        let gap = |pick: &dyn Fn(&Gap) -> f64| gaps.each_ref().map(|gap| gap.as_ref().map(pick));
        let take = |pick: &dyn Fn(&Heard) -> Option<f64>| heard.map(|take| take.and_then(pick));
        let mut rows = vec![(
            "pairDistanceDb",
            "distance from the mic, dB".to_owned(),
            gap(&|g| g.distance_db),
        )];
        for (band, (name, words, _)) in BANDS_READ.iter().enumerate() {
            rows.push((*name, format!("{words}, dB"), gap(&|g| g.bands_db[band])));
        }
        rows.push((
            "sibilanceDb",
            "s over 5 kHz against singing, dB".into(),
            take(&|h| h.sibilance_db),
        ));
        rows.push((
            "plosivesPerMinute",
            "pops under 100 Hz a minute".into(),
            take(&|h| Some(h.plosives_per_minute)),
        ));
        rows.push((
            "noiseFloorDb",
            "room against singing, dB".into(),
            take(&|h| Some(h.noise_floor_db)),
        ));
        rows
    }
}
