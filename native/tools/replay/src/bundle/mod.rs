//! A session bundle read back (docs/design.md 8.11): the song its directory holds, on the
//! replay's own clock, ready for `song::play`.

mod calls;
mod clock;
pub mod line;
pub mod manifest;
pub mod profile;
pub mod report;
mod silence;
pub mod song;
pub mod stems;

#[cfg(test)]
mod live_song;
#[cfg(test)]
mod scorecard_fixture;
#[cfg(test)]
mod tests;

use std::path::Path;

use transport::receive::packet_from;

use crate::capture;
use crate::stream::Arrival;
use clock::Clock;
use line::{Event, Line, Stats, read_lines};
use manifest::Manifest;
use profile::Profile;

pub struct Bundle {
    pub manifest: Manifest,
    /// The profile the song ran under.
    pub profile: Profile,
    /// Each event and the replay frame it is taken up at, in frame order.
    pub events: Vec<(i64, Event)>,
    /// Every recorded slot's packets with their slot, on the replay's frames, in frame order.
    pub arrivals: Vec<(usize, Arrival)>,
    /// Whether the bundle ends in `end`. One without it was cut short.
    pub ended: bool,
}

impl Bundle {
    /// The bundle in `dir`. A format it does not know is refused by name before anything else is
    /// read.
    pub fn read(dir: &Path) -> Result<Self, String> {
        let file = |name: &str| {
            std::fs::read_to_string(dir.join(name)).map_err(|e| format!("{name}: {e}"))
        };
        let manifest = Manifest::parse(&file("manifest.json")?)?;
        let profile = Profile::parse("profile.json", &file("profile.json")?)?;
        let events: Vec<Line<Event>> = read_lines("events.jsonl", &file("events.jsonl")?)?;
        let stats: Vec<Line<Stats>> = read_lines("stats.jsonl", &file("stats.jsonl")?)?;

        let pairs = std::iter::once((manifest.us, manifest.frame))
            .chain(events.iter().map(|line| (line.us, line.frame)))
            .chain(stats.iter().map(|line| (line.us, line.frame)))
            .collect();
        let clock = Clock::new(pairs);
        let origin = manifest.frame;
        let ended = events.last().is_some_and(|line| line.body == Event::End {});
        let mut events: Vec<(i64, Event)> = events
            .into_iter()
            .map(|line| ((line.frame - origin).round() as i64, line.body))
            .collect();
        // The writer puts lines in frame order. One written before #222 put a command's line at
        // its ack, after lines stamped later, and the stable sort puts it back at its stamp
        events.sort_by_key(|&(at, _)| at);

        let mut arrivals = Vec::new();
        for slot in manifest.slots.iter().filter(|slot| slot.recorded) {
            let name = format!("slots/{}.pcap", slot.index);
            let bytes = std::fs::read(dir.join(&name)).map_err(|e| format!("{name}: {e}"))?;
            for datagram in capture::read(&bytes).map_err(|e| format!("{name}: {e}"))? {
                // The bundle's stand-in for the sender: 0.0.0.0 at a port of its own, which the
                // slot's sender lock tells apart as it told the real addresses apart (decision 0028)
                let Some(packet) = packet_from(&datagram.bytes, datagram.source.into()) else {
                    continue;
                };
                let at = clock.frame_at(datagram.at_micros as f64) - origin.round() as i64;
                arrivals.push((slot.index, Arrival { at, packet }));
            }
        }
        arrivals.sort_by_key(|(_, arrival)| arrival.at);
        Ok(Self { manifest, profile, events, arrivals, ended })
    }
}
