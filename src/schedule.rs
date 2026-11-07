//! Time-of-day schedule: which entry is active at a given minute of the day.

/// Parse `HH:MM` (24h) into minutes since midnight.
pub fn parse_hhmm(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.trim().parse().ok()?, m.trim().parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Index of the entry in effect at `now` (minutes since midnight): the latest
/// entry at or before `now`, else the latest one overall (carried over from
/// yesterday). Entries that fail to parse are ignored. Ties: later entry wins.
pub fn active_index(times: &[Option<u32>], now: u32) -> Option<usize> {
    let valid = || {
        times
            .iter()
            .enumerate()
            .filter_map(|(i, t)| t.map(|t| (i, t)))
    };
    let pick = |it: &mut dyn Iterator<Item = (usize, u32)>| {
        it.fold(None, |best: Option<(usize, u32)>, (i, t)| match best {
            Some((_, bt)) if bt > t => best,
            _ => Some((i, t)),
        })
    };
    pick(&mut valid().filter(|&(_, t)| t <= now))
        .or_else(|| pick(&mut valid()))
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hhmm() {
        assert_eq!(parse_hhmm("07:30"), Some(450));
        assert_eq!(parse_hhmm(" 0:00 "), Some(0));
        assert_eq!(parse_hhmm("23:59"), Some(1439));
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_hhmm("12:60"), None);
        assert_eq!(parse_hhmm("noon"), None);
    }

    #[test]
    fn active_entry() {
        let t = [
            parse_hhmm("07:30"),
            parse_hhmm("21:00"),
            parse_hhmm("13:00"),
        ];
        assert_eq!(active_index(&t, 8 * 60), Some(0));
        assert_eq!(active_index(&t, 13 * 60), Some(2));
        assert_eq!(active_index(&t, 22 * 60), Some(1));
        // before first entry of the day -> last entry from yesterday
        assert_eq!(active_index(&t, 3 * 60), Some(1));
        assert_eq!(active_index(&[], 100), None);
        assert_eq!(active_index(&[None, Some(10)], 5), Some(1));
        // tie: later entry wins
        assert_eq!(active_index(&[Some(60), Some(60)], 61), Some(1));
    }
}
