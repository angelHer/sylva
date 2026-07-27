//! The selected commit, in full.

use eframe::egui::{self, RichText, Ui};

use super::theme::Palette;
use crate::domain::{Oid, RepositorySnapshot, Timestamp};

pub struct CommitDetail<'a> {
    pub snapshot: &'a RepositorySnapshot,
    pub selected: Option<Oid>,
}

impl CommitDetail<'_> {
    pub fn show(&self, ui: &mut Ui) {
        let Some(commit) = self.selected.and_then(|id| self.snapshot.commit(&id)) else {
            ui.label(
                RichText::new("Select a commit")
                    .color(Palette::TEXT_FAINT)
                    .size(11.5),
            );
            return;
        };

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.label(
                    RichText::new(commit.summary.clone())
                        .color(Palette::TEXT)
                        .size(13.0)
                        .strong(),
                );
                ui.add_space(6.0);

                // Selectable so the hash can be copied out to a terminal.
                ui.add(
                    egui::Label::new(
                        RichText::new(commit.id.to_hex())
                            .color(Palette::CYAN)
                            .size(10.5)
                            .monospace(),
                    )
                    .selectable(true)
                    .wrap(),
                );

                ui.add_space(10.0);
                field(ui, "author", &format!("{} <{}>", commit.author.name, commit.author.email));
                field(ui, "authored", &format_timestamp(commit.author.at));

                // The committer is only worth showing when it differs — after a
                // rebase or a patch applied on someone's behalf.
                if commit.committer.email != commit.author.email {
                    field(
                        ui,
                        "committer",
                        &format!("{} <{}>", commit.committer.name, commit.committer.email),
                    );
                }
                if commit.committer.at != commit.author.at {
                    field(ui, "committed", &format_timestamp(commit.committer.at));
                }

                let refs: Vec<String> = self
                    .snapshot
                    .branch_tips()
                    .get(&commit.id)
                    .map(|branches| branches.iter().map(|b| b.name.clone()).collect())
                    .unwrap_or_default();
                if !refs.is_empty() {
                    field(ui, "branches", &refs.join(", "));
                }

                let here: Vec<String> = self
                    .snapshot
                    .worktrees_at(&commit.id)
                    .iter()
                    .map(|wt| wt.dir_name().to_string())
                    .collect();
                if !here.is_empty() {
                    field(ui, "worktrees", &here.join(", "));
                }

                if !commit.parents.is_empty() {
                    let parents: Vec<String> = commit
                        .parents
                        .iter()
                        .map(|parent| parent.to_short_hex(8))
                        .collect();
                    field(ui, "parents", &parents.join(", "));
                }

                if !commit.body.is_empty() {
                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(6.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(commit.body.clone())
                                .color(Palette::TEXT_DIM)
                                .size(11.5),
                        )
                        .selectable(true)
                        .wrap(),
                    );
                }
            });
    }
}

fn field(ui: &mut Ui, label: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(label)
                .color(Palette::TEXT_FAINT)
                .size(10.5)
                .monospace(),
        );
        ui.label(RichText::new(value).color(Palette::TEXT_DIM).size(11.0));
    });
}

/// Renders a Git timestamp in the author's own local time, the way Git records
/// it: `2023-11-14 22:13 +00:00`.
///
/// Written out by hand rather than pulling in a date-time crate. Git stores an
/// instant plus a fixed offset — there is no time zone, no daylight saving and
/// no locale involved, so this is arithmetic, not calendar handling.
pub fn format_timestamp(stamp: Timestamp) -> String {
    let local = stamp.local_seconds();

    // Euclidean division so instants before 1970 floor towards the past
    // instead of towards zero.
    let days = local.div_euclid(86_400);
    let seconds = local.rem_euclid(86_400);

    let (year, month, day) = civil_from_days(days);
    let (hour, minute) = (seconds / 3600, (seconds % 3600) / 60);

    let offset = stamp.offset_minutes;
    let sign = if offset < 0 { '-' } else { '+' };
    let (offset_hours, offset_minutes) = (offset.abs() / 60, offset.abs() % 60);

    format!(
        "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02} {sign}{offset_hours:02}:{offset_minutes:02}"
    )
}

/// Days since 1970-01-01 to a calendar date, by Howard Hinnant's `civil_from_days`.
///
/// Shifts the epoch to 0000-03-01 so that February — and therefore the leap
/// day — falls at the end of the year, which removes every special case.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;

    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097; // [0, 146096]

    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;

    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;

    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u32;

    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epoch_is_the_first_of_january_nineteen_seventy() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn the_day_before_the_epoch_is_the_last_of_december_nineteen_sixty_nine() {
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    #[test]
    fn leap_days_are_handled() {
        // 2024 is a leap year, so the 29th of February exists.
        let days = 19_782; // 2024-02-29
        assert_eq!(civil_from_days(days), (2024, 2, 29));
    }

    #[test]
    fn century_years_divisible_by_four_hundred_are_leap_years() {
        // 2000 was a leap year; 1900 was not.
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn a_utc_timestamp_renders_with_a_zero_offset() {
        let stamp = Timestamp::from_utc(1_700_000_000);
        assert_eq!(format_timestamp(stamp), "2023-11-14 22:13 +00:00");
    }

    #[test]
    fn a_negative_offset_shifts_the_clock_back_and_is_shown() {
        // Buenos Aires, UTC-03:00.
        let stamp = Timestamp::new(1_700_000_000, -180);
        assert_eq!(format_timestamp(stamp), "2023-11-14 19:13 -03:00");
    }

    #[test]
    fn a_positive_offset_shifts_the_clock_forward() {
        let stamp = Timestamp::new(1_700_000_000, 120);
        assert_eq!(format_timestamp(stamp), "2023-11-15 00:13 +02:00");
    }

    #[test]
    fn offsets_that_are_not_whole_hours_are_rendered_exactly() {
        // Kathmandu, UTC+05:45.
        let stamp = Timestamp::new(1_700_000_000, 345);
        assert_eq!(format_timestamp(stamp), "2023-11-15 03:58 +05:45");
    }

    #[test]
    fn an_instant_before_the_epoch_does_not_wrap_into_the_future() {
        let stamp = Timestamp::from_utc(-1);
        assert_eq!(format_timestamp(stamp), "1969-12-31 23:59 +00:00");
    }
}
