//! Next semester's website.
//!
//! The other parser tests pin the parser to the pages CMI published on 5 Aug
//! 2026. These build pages CMI has *not* published — a January term, other
//! slot times, halls added and removed, branches that come and go, codes in
//! shapes nobody has used yet — and hold the parser to the same promises.
//! Nothing about the site may be hard-coded, so a page that looks nothing
//! like today's must still read correctly, while a page that is merely
//! broken must still be refused.
//!
//! `site::Site` renders both pages from a compact description, reproducing
//! the real HTML's shape down to its quirks: grid rows wrapped in `<b>`, day
//! sections wrapped in `<div>`/`<a>`, and the hall header's label cell one
//! character narrower than the hall rows beneath it.

use cmi_timetable_core::extract::parse_html_pages;
use cmi_timetable_core::model::{Day, ScheduleStatus, Snapshot, SourceTier};
use cmi_timetable_core::validate::ParseOutcome;
use site::Site;

// ---------------------------------------------------------------------------
// The site builder
// ---------------------------------------------------------------------------

mod site {
    /// One branch grid. Its legend block is rendered from the site's course
    /// table, restricted to the codes this grid actually uses — which is how
    /// the real page works.
    #[derive(Clone, Default)]
    pub struct Branch {
        pub code: String,
        pub title: String,
        /// (day label as printed, one cell per slot column).
        pub rows: Vec<(String, Vec<String>)>,
        /// Column headers for this grid alone, when it does not use the
        /// site's. CMI prints one header per branch, so they can differ.
        pub slots: Option<Vec<String>>,
    }

    /// A whole publication: timetable.php and lecturehalls.php.
    #[derive(Clone, Default)]
    pub struct Site {
        pub semester: Option<String>,
        pub halls_semester: Option<String>,
        /// Column headers verbatim — "09:10-10:25", "9.10 to 10.25", "2:00pm-3:15pm".
        pub slots: Vec<String>,
        pub branches: Vec<Branch>,
        pub halls: Vec<String>,
        pub hall_days: Vec<String>,
        /// (day label, hall, column index, cell text).
        pub bookings: Vec<(String, String, usize, String)>,
        /// code -> (name, instructor). Rendered into both pages' legends.
        pub courses: Vec<(String, String, String)>,
        /// Codes deliberately left out of the legends.
        pub unlisted: Vec<String>,
        pub footnotes: Vec<String>,
        /// Print the hall header's label cell one character narrower than the
        /// hall rows', exactly as the live page does.
        pub ragged_hall_header: bool,
        /// Drop every `|` from the hall grid (a re-typed page).
        pub pipeless_halls: bool,
        /// Verbatim edits applied to the finished page, for the things a
        /// builder cannot express: a day line typed with a date after it, a
        /// stray separator in one row, a code printed in the wrong case.
        /// (from, to), applied in order, and each one must actually match —
        /// a test whose edit silently missed would pass for the wrong
        /// reason.
        pub tt_edits: Vec<(String, String)>,
        pub halls_edits: Vec<(String, String)>,
    }

    fn s(x: &str) -> String {
        x.to_string()
    }

    /// Apply verbatim edits to a rendered page, insisting each one matches.
    /// A test whose edit quietly missed would still pass, and would be
    /// testing nothing at all.
    fn apply(edits: &[(String, String)], mut html: String) -> String {
        for (from, to) in edits {
            assert!(
                html.contains(from.as_str()),
                "the page has no {from:?} to retype — the test edit missed"
            );
            html = html.replace(from.as_str(), to.as_str());
        }
        html
    }

    impl Site {
        /// A site whose timetable page announces `semester`. The halls page
        /// carries no label, as CMI's really doesn't.
        pub fn new(semester: &str) -> Site {
            Site {
                semester: Some(s(semester)),
                ragged_hall_header: true,
                ..Site::default()
            }
        }

        pub fn unlabelled() -> Site {
            Site {
                ragged_hall_header: true,
                ..Site::default()
            }
        }

        pub fn halls_semester(mut self, label: &str) -> Site {
            self.halls_semester = Some(s(label));
            self
        }

        pub fn slots(mut self, slots: &[&str]) -> Site {
            self.slots = slots.iter().map(|x| s(x)).collect();
            self
        }

        /// Add a branch grid. Each row is (day label, cells), one cell per
        /// slot column; a short row is padded with blanks.
        pub fn branch(mut self, code: &str, title: &str, rows: &[(&str, &[&str])]) -> Site {
            self.branches.push(Branch {
                code: s(code),
                title: s(title),
                rows: rows
                    .iter()
                    .map(|(day, cells)| (s(day), cells.iter().map(|c| s(c)).collect()))
                    .collect(),
                slots: None,
            });
            self
        }

        /// Give the branch just added its own column headers.
        pub fn own_columns(mut self, slots: &[&str]) -> Site {
            let own = slots.iter().map(|x| s(x)).collect();
            self.branches
                .last_mut()
                .expect("own_columns follows a branch")
                .slots = Some(own);
            self
        }

        pub fn course(mut self, code: &str, name: &str, instructor: &str) -> Site {
            self.courses.push((s(code), s(name), s(instructor)));
            self
        }

        /// A code that appears in a grid but in no legend.
        pub fn unlisted(mut self, code: &str) -> Site {
            self.unlisted.push(s(code));
            self
        }

        pub fn halls(mut self, halls: &[&str]) -> Site {
            self.halls = halls.iter().map(|x| s(x)).collect();
            self
        }

        pub fn hall_days(mut self, days: &[&str]) -> Site {
            self.hall_days = days.iter().map(|x| s(x)).collect();
            self
        }

        pub fn book(mut self, day: &str, hall: &str, col: usize, cell: &str) -> Site {
            self.bookings.push((s(day), s(hall), col, s(cell)));
            self
        }

        pub fn footnote(mut self, text: &str) -> Site {
            self.footnotes.push(s(text));
            self
        }

        /// Listed-but-unscheduled courses, of which every real CMI semester
        /// has a handful. Used to lift a toy site over the gate's course
        /// floor without inventing more grids.
        pub fn reading_courses(mut self, n: usize) -> Site {
            for i in 1..=n {
                self.courses.push((
                    format!("RC{i:02}"),
                    format!("Reading Course {i}"),
                    s("Various"),
                ));
            }
            self
        }

        /// Retype one piece of the timetable page verbatim.
        pub fn retype_timetable(mut self, from: &str, to: &str) -> Site {
            self.tt_edits.push((s(from), s(to)));
            self
        }

        /// Retype one piece of the halls page verbatim.
        pub fn retype_halls(mut self, from: &str, to: &str) -> Site {
            self.halls_edits.push((s(from), s(to)));
            self
        }

        pub fn pipeless_halls(mut self) -> Site {
            self.pipeless_halls = true;
            self
        }

        // -- what a new semester does to last semester's page ---------------

        pub fn relabel(mut self, semester: &str) -> Site {
            self.semester = Some(s(semester));
            self
        }

        pub fn drop_branch(mut self, code: &str) -> Site {
            self.branches.retain(|b| b.code != code);
            self
        }

        /// Take a course off the site entirely: its legend entry, its cells
        /// in every grid, and its hall bookings.
        pub fn drop_course(mut self, code: &str) -> Site {
            self.courses.retain(|(c, _, _)| c != code);
            for branch in &mut self.branches {
                for (_, cells) in &mut branch.rows {
                    for cell in cells.iter_mut() {
                        if mentions(cell, code) {
                            *cell = String::new();
                        }
                    }
                }
            }
            self.bookings
                .retain(|(_, _, _, cell)| !mentions(cell, code));
            self
        }

        /// Move a class to another column of its branch grid.
        pub fn move_class(mut self, branch: &str, day: &str, code: &str, to_col: usize) -> Site {
            let slots = self.slots.len();
            let Some(b) = self.branches.iter_mut().find(|b| b.code == branch) else {
                panic!("no branch {branch}");
            };
            let Some((_, cells)) = b.rows.iter_mut().find(|(d, _)| d == day) else {
                panic!("branch {branch} has no {day} row");
            };
            cells.resize(slots, String::new());
            let from = cells
                .iter()
                .position(|c| mentions(c, code))
                .unwrap_or_else(|| panic!("{code} is not in {branch}'s {day} row"));
            let text = std::mem::take(&mut cells[from]);
            cells[to_col] = text;
            self
        }

        /// Move a course's booking on one day into another hall and column.
        pub fn move_booking(mut self, day: &str, code: &str, hall: &str, col: usize) -> Site {
            self.bookings
                .retain(|(d, _, _, cell)| !(d == day && mentions(cell, code)));
            self.bookings.push((s(day), s(hall), col, s(code)));
            self
        }
    }

    /// Does this grid cell name that course? Cells hold whitespace- or
    /// slash-separated codes, optionally suffixed with '+'.
    fn mentions(cell: &str, code: &str) -> bool {
        cell.split(['/', ' '])
            .any(|t| t.trim().trim_end_matches('+') == code)
    }

    impl Site {
        /// The columns a branch grid prints: its own, or the site's.
        fn columns_of<'a>(&'a self, branch: &'a Branch) -> &'a [String] {
            branch.slots.as_deref().unwrap_or(&self.slots)
        }

        /// Width of one slot column, wide enough for the longest header
        /// anywhere on the site so every grid lines up the same way.
        fn cell_width(&self) -> usize {
            self.slots
                .iter()
                .chain(self.branches.iter().flat_map(|b| b.slots.iter().flatten()))
                .map(|s| s.chars().count())
                .max()
                .unwrap_or(9)
                .max(9)
        }

        fn header_cells(&self, slots: &[String]) -> String {
            let w = self.cell_width();
            slots
                .iter()
                .map(|s| format!("{s:<w$}"))
                .collect::<Vec<_>>()
                .join("|")
        }

        fn data_cells(&self, slots: &[String], cells: &[String]) -> String {
            let w = self.cell_width();
            (0..slots.len())
                .map(|i| {
                    let text = cells.get(i).map(String::as_str).unwrap_or("");
                    let inner = w.saturating_sub(2);
                    format!("  {text:<inner$}")
                })
                .collect::<Vec<_>>()
                .join("|")
        }

        fn blank_cells(&self) -> String {
            self.data_cells(&self.slots, &[])
        }

        // -- timetable.php ------------------------------------------------

        fn legend_for(&self, branch: &Branch) -> String {
            let used: Vec<&String> = self
                .courses
                .iter()
                .map(|(code, _, _)| code)
                .filter(|code| {
                    branch.rows.iter().any(|(_, cells)| {
                        cells.iter().any(|c| {
                            c.split(['/', ' '])
                                .any(|t| t.trim_end_matches('+') == code.as_str())
                        })
                    })
                })
                .collect();
            let width = used.iter().map(|c| c.chars().count()).max().unwrap_or(4);
            let name_width = self
                .courses
                .iter()
                .map(|(_, n, _)| n.chars().count())
                .max()
                .unwrap_or(20)
                .max(20);
            used.iter()
                .map(|code| {
                    let (_, name, instr) = self
                        .courses
                        .iter()
                        .find(|(c, _, _)| c == *code)
                        .expect("legend entry");
                    // Name and instructor are separated by a run of ≥2 spaces,
                    // as the timetable page's legend does it.
                    format!("{code:<width$}: {name:<name_width$}  {instr}")
                })
                .collect::<Vec<_>>()
                .join("\n")
        }

        pub fn timetable_html(&self) -> String {
            let mut body = String::new();
            for branch in &self.branches {
                let slots = self.columns_of(branch);
                let header = self.header_cells(slots);
                let label_width = branch
                    .rows
                    .iter()
                    .map(|(d, _)| d.chars().count())
                    .chain(std::iter::once(branch.code.chars().count()))
                    .max()
                    .unwrap_or(3);
                let rule_len = label_width + 2 + (self.cell_width() + 1) * slots.len();
                let eq = "=".repeat(rule_len);
                let eq_split = format!(
                    "{}+{}+",
                    "=".repeat(label_width + 2),
                    vec!["=".repeat(self.cell_width()); slots.len()].join("+")
                );

                body.push_str("<pre>\n\n");
                if let Some(sem) = &self.semester {
                    body.push_str(&format!("<b>Timetable for {sem}\n</b>\n"));
                }
                body.push_str(&format!("<b>{}</b>\n\n", branch.title));
                body.push_str(&format!("<b>{eq}\n</b>"));
                body.push_str(&format!(
                    "<b> {:<w$} |{header}|</b>\n",
                    branch.code,
                    w = label_width
                ));
                body.push_str(&format!("<b>{eq_split}\n</b>"));
                for (day, cells) in &branch.rows {
                    body.push_str(&format!(
                        " <b>{:<w$}</b> |{}|\n",
                        day,
                        self.data_cells(slots, cells),
                        w = label_width
                    ));
                }
                body.push_str(&format!("<b>{eq}\n</b></pre>\n"));

                let legend = self.legend_for(branch);
                if !legend.is_empty() {
                    body.push_str(&format!("<pre>{legend}\n</pre>\n"));
                }
                body.push_str("<pre>\n+ Optional course.\n</pre>\n");
            }
            apply(&self.tt_edits, page("Timetable", &body))
        }

        // -- lecturehalls.php ---------------------------------------------

        pub fn halls_html(&self) -> String {
            let label_width = self
                .halls
                .iter()
                .map(|h| h.chars().count() + 3)
                .chain(self.hall_days.iter().map(|d| d.chars().count()))
                .max()
                .unwrap_or(18)
                .max(18);
            let sep = format!(
                "{}+{}+",
                "-".repeat(label_width),
                vec!["-".repeat(self.cell_width()); self.slots.len()].join("+")
            );
            let cut = |line: String| -> String {
                if self.pipeless_halls {
                    line.replace('|', " ")
                } else {
                    line
                }
            };

            let mut grid = String::new();
            grid.push('\n');
            grid.push_str(&"-".repeat(sep.chars().count()));
            grid.push('\n');
            // The header's label cell is one character narrower than the hall
            // rows' — a real quirk of the live page, kept on by default.
            let head_pad = label_width - usize::from(self.ragged_hall_header);
            grid.push_str(&cut(format!(
                "{}|{}|",
                " ".repeat(head_pad),
                self.header_cells(&self.slots)
            )));
            grid.push('\n');
            grid.push_str(&cut(sep.clone()));
            grid.push('\n');

            for day in &self.hall_days {
                let id = day.to_ascii_lowercase();
                let id: String = id.chars().take(3).collect();
                grid.push_str(&format!(
                    "<div id=\"{id}\" style=\"display:block;\" ><a onclick=\"toggle('{id}');\" >"
                ));
                grid.push_str(&cut(format!(
                    "{:<w$}</a>|{}|",
                    day,
                    self.blank_cells(),
                    w = label_width
                )));
                grid.push('\n');
                for hall in &self.halls {
                    let cells: Vec<String> = (0..self.slots.len())
                        .map(|col| {
                            self.bookings
                                .iter()
                                .find(|(d, h, c, _)| d == day && h == hall && *c == col)
                                .map(|(_, _, _, text)| text.clone())
                                .unwrap_or_default()
                        })
                        .collect();
                    grid.push_str(&cut(format!(
                        "   {:<w$}|{}|",
                        hall,
                        self.data_cells(&self.slots, &cells),
                        w = label_width - 3
                    )));
                    grid.push('\n');
                }
                grid.push_str(&cut(sep.clone()));
                grid.push_str("\n</div>");
            }

            let mut body = String::new();
            body.push_str("<pre>");
            if let Some(sem) = &self.halls_semester {
                body.push_str(&format!("Timetable for {sem}\n"));
            }
            body.push_str(&grid);
            for note in &self.footnotes {
                body.push_str(&format!("\n{note}"));
            }
            body.push_str("\n</pre>\n");

            let width = self
                .courses
                .iter()
                .map(|(c, _, _)| c.chars().count())
                .max()
                .unwrap_or(4);
            let name_width = self
                .courses
                .iter()
                .map(|(_, n, _)| n.chars().count())
                .max()
                .unwrap_or(20);
            let legend: Vec<String> = self
                .courses
                .iter()
                .map(|(code, name, instr)| {
                    format!("{code:<width$} : {name:<name_width$} : {instr}")
                })
                .collect();
            if !legend.is_empty() {
                body.push_str(&format!("<pre>{}\n</pre>\n", legend.join("\n")));
            }
            apply(&self.halls_edits, page("Lecture Hall Allocation", &body))
        }

        /// Parse both pages exactly as the app does.
        pub fn read(&self) -> super::ParseOutcome {
            super::parse_html_pages(
                &self.timetable_html(),
                &self.halls_html(),
                0.0,
                super::SourceTier::Bundled,
                false,
            )
        }
    }

    /// The surrounding page furniture — menus, headings, footer. None of it
    /// carries data; all of it is there to be ignored.
    fn page(subject: &str, body: &str) -> String {
        format!(
            "<html>\n<head>\n<title>Chennai Mathematical Institute</title>\n\
             <link rel=\"stylesheet\" href=\"https://www.cmi.ac.in//basic.css\"/></head>\n\
             <body bgcolor=\"white\">\n\
             <div id=\"main_menu_box\"><ul class=\"main_menu\">\n\
             <li><a href=\"https://www.cmi.ac.in//\">Home</a></li>\n\
             <li><a href=\"https://www.cmi.ac.in/teaching/\">Teaching</a></li>\n\
             </ul></div>\n\
             <div id=\"page_subject\">\n{subject}</div>\n<br>\n\
             {body}\n\
             <div id=\"footer\">Chennai Mathematical Institute</div>\n\
             </body>\n</html>\n"
        )
    }
}

// ---------------------------------------------------------------------------
// Shared scenarios
// ---------------------------------------------------------------------------

/// A plausible January term: fewer branches than August, other slot times,
/// a hall list that has moved on, and a mix of full and short day names.
fn january_term() -> Site {
    Site::new("January--April 2027")
        .slots(&[
            "08:30-09:45",
            "10:00-11:15",
            "11:30-12:45",
            "14:30-15:45",
            "16:00-17:15",
        ])
        .branch(
            "BM1",
            "B.S I year",
            &[
                ("Mon", &["CAL1", "", "PROG", "", "ENG"]),
                ("Tue", &["", "CAL1", "", "PROG", ""]),
                ("Wed", &["CAL1", "", "ENG", "", ""]),
                ("Thu", &["", "PROG", "", "CAL1", ""]),
                ("Fri", &["", "", "ENG", "", "SEM"]),
            ],
        )
        .branch(
            "BM2",
            "B.S. Mathematics & CS II year",
            &[
                ("Mon", &["", "TOPO", "", "COMB", ""]),
                ("Tue", &["TOPO", "", "COMB", "", ""]),
                ("Wed", &["", "TOPO", "", "", "COMB"]),
                ("Thu", &["COMB", "", "TOPO", "", ""]),
                ("Fri", &["", "", "", "COMB+", ""]),
            ],
        )
        .branch(
            "MC1",
            "M.Sc Computer Science I year",
            &[
                ("Monday", &["", "", "CRYP", "", "AIML"]),
                ("Tuesday", &["AIML", "", "", "CRYP", ""]),
                ("Wednesday", &["", "CRYP", "", "", "AIML"]),
                ("Thursday", &["AIML", "", "CRYP", "", ""]),
            ],
        )
        .branch(
            "OP1",
            "Phy Electives [PhD]",
            &[
                ("Mon", &["", "QFT", "", "", ""]),
                ("Wed", &["QFT", "", "", "", "GRAV"]),
                ("Fri", &["", "GRAV", "", "QFT", ""]),
            ],
        )
        .course("CAL1", "Calculus I", "Manoj Kummini")
        .course("PROG", "Introduction to Programming(Haskell)", "S P Suresh")
        .course("ENG", "English", "Usha Mahadevan")
        .course("SEM", "Student Seminar", "Speaker")
        .course("TOPO", "Topology", "P Sankaran")
        .course("COMB", "Combinatorics", "Prajakta Nimbhorkar")
        .course("CRYP", "Introduction to Cryptography", "Varun Narayanan")
        .course("AIML", "AI and Machine Learning", "K V Subrahmanyam")
        .course("QFT", "Quantum Field Theory", "Alok Laddha")
        .course("GRAV", "General Relativity", "V V Sreedhar")
        .course("RDNG", "Reading Course(Feb-Apr)", "Various")
        .course("LABW", "Physics Lab", "K G M Nair")
        .halls(&[
            "Seminar Hall",
            "Lecture Hall 1",
            "Lecture Hall 2",
            "Lecture Hall 205",
            "Lecture Hall 801",
            "Physics Lab",
            "Auditorium",
        ])
        .hall_days(&["Monday", "Tuesday", "Wednesday", "Thursday", "Friday"])
        .book("Monday", "Lecture Hall 1", 0, "CAL1")
        .book("Monday", "Lecture Hall 2", 1, "TOPO")
        .book("Monday", "Lecture Hall 205", 2, "PROG")
        .book("Monday", "Lecture Hall 801", 3, "COMB")
        .book("Monday", "Auditorium", 4, "ENG")
        .book("Monday", "Seminar Hall", 1, "QFT")
        .book("Tuesday", "Lecture Hall 1", 1, "CAL1")
        .book("Tuesday", "Lecture Hall 2", 0, "TOPO")
        .book("Tuesday", "Lecture Hall 205", 3, "PROG")
        .book("Tuesday", "Lecture Hall 801", 2, "COMB")
        .book("Tuesday", "Auditorium", 0, "AIML")
        .book("Tuesday", "Seminar Hall", 3, "CRYP")
        .book("Wednesday", "Lecture Hall 1", 0, "CAL1")
        .book("Wednesday", "Lecture Hall 2", 1, "TOPO")
        .book("Wednesday", "Lecture Hall 205", 2, "ENG")
        .book("Wednesday", "Lecture Hall 801", 4, "COMB")
        .book("Wednesday", "Auditorium", 4, "AIML")
        .book("Wednesday", "Seminar Hall", 0, "QFT")
        .book("Wednesday", "Physics Lab", 1, "LABW")
        .book("Thursday", "Lecture Hall 1", 3, "CAL1")
        .book("Thursday", "Lecture Hall 2", 2, "TOPO")
        .book("Thursday", "Lecture Hall 205", 1, "PROG")
        .book("Thursday", "Lecture Hall 801", 0, "COMB")
        .book("Thursday", "Auditorium", 0, "AIML")
        .book("Thursday", "Seminar Hall", 2, "CRYP")
        .book("Friday", "Lecture Hall 1", 4, "SEM")
        .book("Friday", "Lecture Hall 205", 2, "ENG")
        .book("Friday", "Lecture Hall 801", 3, "COMB")
        .book("Friday", "Seminar Hall", 1, "GRAV")
        .book("Friday", "Auditorium", 3, "QFT")
        .book("Friday", "Physics Lab", 0, "LABW TMP*")
        .footnote("*Note: Lecture Hall 205 (2nd floor) in the new building.")
        .footnote("*Temporary booking of lecture halls are marked as 'TMP*'.")
}

/// The term after [`january_term`]: the second-year branch is gone and took
/// its two courses with it, Calculus has moved an hour later and across the
/// corridor, everything else is exactly where it was, and there is a new
/// branch teaching a new course.
fn the_term_after() -> Site {
    january_term()
        .relabel("August--November 2027")
        .drop_branch("BM2")
        .drop_course("TOPO")
        .drop_course("COMB")
        .move_class("BM1", "Mon", "CAL1", 1)
        .move_booking("Monday", "CAL1", "Lecture Hall 2", 1)
        .branch(
            "DS1",
            "M.Sc Data Science I year",
            &[
                ("Mon", &["", "", "DSCI", "", ""]),
                ("Wed", &["DSCI", "", "", "", ""]),
                ("Fri", &["", "DSCI", "", "", ""]),
            ],
        )
        .course("DSCI", "Foundations of Data Science", "A Newcomer")
        .book("Monday", "Lecture Hall 205", 2, "DSCI")
}

fn snapshot_of(out: &ParseOutcome) -> &Snapshot {
    assert!(
        out.report.gate_passed(),
        "gate should pass: {:#?}\nerrors: {:#?}",
        out.report.gate,
        out.report.errors
    );
    out.snapshot
        .as_ref()
        .expect("snapshot when the gate passes")
}

fn failed_rules(out: &ParseOutcome) -> Vec<String> {
    out.report
        .gate
        .iter()
        .filter(|c| !c.passed)
        .map(|c| c.rule.clone())
        .collect()
}

/// Slot labels are printed with an en dash. The tests below write plain
/// hyphens, which read better in a source file; `a_january_term_reads_like_
/// any_other` pins the real punctuation once so nothing here hides a change.
fn plain(slot: &cmi_timetable_core::model::Slot) -> String {
    slot.label().replace('\u{2013}', "-")
}

fn meeting_days(snap: &Snapshot, code: &str) -> Vec<(Day, String, Option<String>)> {
    let mut v: Vec<(Day, String, Option<String>)> = snap
        .course(code)
        .unwrap_or_else(|| panic!("{code} missing from the snapshot"))
        .meetings
        .iter()
        .map(|m| (m.day, plain(&m.slot), m.hall.clone()))
        .collect();
    v.sort();
    v
}

// ---------------------------------------------------------------------------
// A term the parser has never seen
// ---------------------------------------------------------------------------

/// A January--April term with other slot times, other halls and other
/// courses reads exactly as August does. Nothing about the current
/// semester may be baked in.
#[test]
fn a_january_term_reads_like_any_other() {
    let out = january_term().read();
    let snap = snapshot_of(&out);

    assert_eq!(snap.semester_label, "January--April 2027");
    assert_eq!(snap.semester_label_display(), "January\u{2013}April 2027");
    assert_eq!(snap.branches.len(), 4);
    assert_eq!(
        snap.branch("MC1").unwrap().title,
        "M.Sc Computer Science I year"
    );

    // Slot columns come from the page, not from a table of "CMI's slots".
    let labels: Vec<String> = snap.slot_grid.iter().map(plain).collect();
    assert_eq!(
        labels,
        [
            "08:30-09:45",
            "10:00-11:15",
            "11:30-12:45",
            "14:30-15:45",
            "16:00-17:15"
        ]
    );
    assert_eq!(
        snap.slot_grid[0].label(),
        "08:30\u{2013}09:45",
        "times are printed with an en dash"
    );

    // Every course in the legend is known, including the two that are listed
    // but never scheduled.
    assert_eq!(
        snap.courses.len(),
        12,
        "{:?}",
        snap.courses.iter().map(|c| &c.code).collect::<Vec<_>>()
    );
    assert_eq!(snap.course("CAL1").unwrap().name, "Calculus I");
    assert_eq!(snap.course("CAL1").unwrap().instructors, ["Manoj Kummini"]);
    assert_eq!(
        snap.course("RDNG").unwrap().status,
        ScheduleStatus::UnscheduledListed
    );

    // A course taught to two branches is one course with both branches.
    let toc = snap.course("TOPO").unwrap();
    assert_eq!(toc.branches, ["BM2"]);
    assert_eq!(snap.course("CRYP").unwrap().branches, ["MC1"]);

    // Meetings land on the right day, in the right slot, in the right hall.
    assert_eq!(
        meeting_days(snap, "CAL1"),
        [
            (
                Day::Mon,
                "08:30-09:45".into(),
                Some("Lecture Hall 1".into())
            ),
            (
                Day::Tue,
                "10:00-11:15".into(),
                Some("Lecture Hall 1".into())
            ),
            (
                Day::Wed,
                "08:30-09:45".into(),
                Some("Lecture Hall 1".into())
            ),
            (
                Day::Thu,
                "14:30-15:45".into(),
                Some("Lecture Hall 1".into())
            ),
        ]
    );

    // Full day names parse as the same days as the short forms — and a class
    // the branch grid lists but the halls page never rooms simply has no room.
    assert_eq!(
        meeting_days(snap, "AIML"),
        [
            (Day::Mon, "16:00-17:15".into(), None),
            (Day::Tue, "08:30-09:45".into(), Some("Auditorium".into())),
            (Day::Wed, "16:00-17:15".into(), Some("Auditorium".into())),
            (Day::Thu, "08:30-09:45".into(), Some("Auditorium".into())),
        ]
    );

    // The optional marker survives.
    let comb = snap.course("COMB").unwrap();
    assert!(comb.meetings.iter().any(|m| m.day == Day::Fri));

    // Halls are whatever the page lists, in page order.
    assert_eq!(
        snap.halls,
        [
            "Seminar Hall",
            "Lecture Hall 1",
            "Lecture Hall 2",
            "Lecture Hall 205",
            "Lecture Hall 801",
            "Physics Lab",
            "Auditorium",
        ]
    );

    // A temporary booking is marked as one.
    let tmp: Vec<&cmi_timetable_core::model::HallBooking> =
        snap.hall_bookings.iter().filter(|b| b.temp).collect();
    assert_eq!(tmp.len(), 1, "one TMP* booking: {tmp:?}");
    assert_eq!(tmp[0].hall, "Physics Lab");
    assert_eq!(tmp[0].day, Day::Fri);
}

/// Halls appear, disappear and get renamed between semesters. The hall list
/// and every booking must follow the page, with no memory of last term.
#[test]
fn halls_come_and_go() {
    let base = january_term();
    let before = base.read();
    let before = snapshot_of(&before);
    assert!(before.halls.iter().any(|h| h == "Lecture Hall 205"));
    assert!(before.halls.iter().any(|h| h == "Physics Lab"));

    // Next term: 205 is gone, the Physics Lab is renamed, two halls are new.
    let after = january_term()
        .halls(&[
            "Seminar Hall",
            "Lecture Hall 1",
            "Lecture Hall 2",
            "Lecture Hall 801",
            "Lab Block A",
            "Auditorium",
            "Ramanujan Hall",
            "Room 12B",
        ])
        .book("Wednesday", "Lab Block A", 1, "LABW")
        .book("Monday", "Ramanujan Hall", 0, "SEM")
        .book("Tuesday", "Room 12B", 4, "RDNG");
    let out = after.read();
    let snap = snapshot_of(&out);

    assert!(!snap.halls.iter().any(|h| h == "Lecture Hall 205"));
    assert!(!snap.halls.iter().any(|h| h == "Physics Lab"));
    assert!(snap.halls.iter().any(|h| h == "Ramanujan Hall"));
    assert!(snap.halls.iter().any(|h| h == "Room 12B"));

    // The renamed hall carries the booking that used to be in the old one.
    assert!(
        snap.hall_bookings
            .iter()
            .any(|b| b.hall == "Lab Block A" && b.codes.iter().any(|c| c == "LABW")),
        "the lab booking follows the rename"
    );
    // A course that was listed-only last term is scheduled now.
    assert_eq!(
        snap.course("RDNG").unwrap().status,
        ScheduleStatus::ScheduledNoBranch,
        "scheduled only via the hall grid"
    );
    assert_eq!(
        meeting_days(snap, "RDNG"),
        [(Day::Tue, "16:00-17:15".into(), Some("Room 12B".into()))]
    );
    // Bookings that named a hall which no longer exists are simply not there.
    assert!(!snap.hall_bookings.iter().any(|b| b.hall == "Physics Lab"));
}

/// A small term — three branches, ten courses, four slots — is a real
/// semester, not garbage. The gate's floors are there to catch an error
/// page, and must not reject a genuinely small January.
#[test]
fn a_minisemester_is_not_garbage() {
    let out = Site::new("January--February 2027")
        .slots(&["09:00-10:15", "10:30-11:45", "12:00-13:15", "14:00-15:15"])
        .branch(
            "MO",
            "Maths Electives",
            &[
                ("Mon", &["MEA", "", "MEB", ""]),
                ("Wed", &["", "MEA", "", "MEB"]),
                ("Fri", &["MEC", "", "", ""]),
            ],
        )
        .branch(
            "OCS1",
            "CS Electives [PhD]",
            &[
                ("Mon", &["", "CSA", "", "CSB"]),
                ("Tue", &["CSA", "", "CSB", ""]),
                ("Thu", &["", "CSC", "", ""]),
            ],
        )
        .branch(
            "OP1",
            "Phy Electives [PhD]",
            &[
                ("Tue", &["", "PHA", "", ""]),
                ("Wed", &["PHA", "", "PHB", ""]),
                ("Fri", &["", "", "", "PHB"]),
            ],
        )
        .course("MEA", "Modular Forms", "B Ramakrishnan")
        .course("MEB", "Lie Algebras", "S Senthamarai Kannan")
        .course("MEC", "Matroid Theory", "Amit Sinhababu")
        .course("CSA", "Complexity Theory", "Partha Mukhopadhyay")
        .course("CSB", "Graduate Algorithms", "Samir Datta")
        .course("CSC", "Program Verification", "M Praveen")
        .course("PHA", "String Theory", "Amitabh Virmani")
        .course("PHB", "Cosmology", "Ajit Mehta")
        .course("SMA", "Maths Seminar", "Speaker")
        .course("SPH", "Physics Seminar", "Speaker")
        .halls(&["Lecture Hall 1", "Lecture Hall 2", "Seminar Hall"])
        .hall_days(&["Monday", "Tuesday", "Wednesday", "Thursday", "Friday"])
        .book("Monday", "Lecture Hall 1", 0, "MEA")
        .book("Monday", "Lecture Hall 2", 1, "CSA")
        .book("Monday", "Seminar Hall", 2, "MEB")
        .book("Monday", "Lecture Hall 1", 3, "CSB")
        .book("Tuesday", "Lecture Hall 1", 0, "CSA")
        .book("Tuesday", "Lecture Hall 2", 1, "PHA")
        .book("Tuesday", "Seminar Hall", 2, "CSB")
        .book("Wednesday", "Lecture Hall 1", 0, "PHA")
        .book("Wednesday", "Lecture Hall 2", 1, "MEA")
        .book("Wednesday", "Seminar Hall", 2, "PHB")
        .book("Wednesday", "Lecture Hall 1", 3, "MEB")
        .book("Thursday", "Lecture Hall 2", 1, "CSC")
        .book("Friday", "Lecture Hall 1", 0, "MEC")
        .book("Friday", "Lecture Hall 2", 3, "PHB")
        .read();
    let snap = snapshot_of(&out);
    assert_eq!(snap.branches.len(), 3);
    assert_eq!(snap.courses.len(), 10);
    assert_eq!(snap.slot_grid.len(), 4);
    assert_eq!(snap.halls.len(), 3);
}

// ---------------------------------------------------------------------------
// Slot columns
// ---------------------------------------------------------------------------

/// The times themselves are data. Dots, "to", am/pm, a range crossing noon
/// and an evening class all have to land in the column they are printed in.
#[test]
fn the_clock_is_read_from_the_page() {
    let out = Site::new("July--November 2028")
        .slots(&[
            "8.00-9.15",
            "9:30 to 10:45",
            "11:50-1:05",
            "2:00pm-3:15pm",
            "6:30-7:45",
        ])
        .branch(
            "BM1",
            "B.S I year",
            &[
                ("Mon", &["EARL", "", "NOON", "", "EVEN"]),
                ("Tue", &["", "MIDM", "", "AFTN", ""]),
                ("Wed", &["EARL", "", "", "", "EVEN"]),
            ],
        )
        .branch(
            "BM2",
            "B.S II year",
            &[
                ("Mon", &["", "MIDM", "", "AFTN", ""]),
                ("Wed", &["", "", "NOON", "", ""]),
                ("Thu", &["EARL", "", "", "AFTN", ""]),
            ],
        )
        .branch(
            "MC1",
            "M.Sc CS",
            &[
                ("Tue", &["EARL", "", "", "", "EVEN"]),
                ("Wed", &["", "MIDM", "", "", ""]),
                ("Fri", &["", "", "NOON", "AFTN", ""]),
            ],
        )
        .course("EARL", "Early Class", "A Teacher")
        .course("MIDM", "Mid Morning", "B Teacher")
        .course("NOON", "Around Noon", "C Teacher")
        .course("AFTN", "Afternoon", "D Teacher")
        .course("EVEN", "Evening Class", "E Teacher")
        .reading_courses(6)
        .halls(&["Lecture Hall 1", "Lecture Hall 2", "Lecture Hall 3"])
        .hall_days(&["Monday", "Tuesday", "Wednesday", "Thursday", "Friday"])
        .book("Monday", "Lecture Hall 1", 0, "EARL")
        .book("Monday", "Lecture Hall 2", 2, "NOON")
        .book("Monday", "Lecture Hall 3", 4, "EVEN")
        .book("Tuesday", "Lecture Hall 1", 1, "MIDM")
        .book("Tuesday", "Lecture Hall 2", 3, "AFTN")
        .book("Wednesday", "Lecture Hall 1", 0, "EARL")
        .book("Wednesday", "Lecture Hall 3", 4, "EVEN")
        .read();
    let snap = snapshot_of(&out);

    let labels: Vec<String> = snap.slot_grid.iter().map(plain).collect();
    assert_eq!(
        labels,
        [
            "08:00-09:15", // dot minutes
            "09:30-10:45", // "to" instead of a dash
            "11:50-13:05", // crosses noon: the end is afternoon
            "14:00-15:15", // explicit pm
            "18:30-19:45", // bare afternoon hours are evening, not dawn
        ]
    );
    assert_eq!(
        meeting_days(snap, "EVEN")
            .into_iter()
            .map(|(d, s, _)| (d, s))
            .collect::<Vec<_>>(),
        [
            (Day::Mon, "18:30-19:45".to_string()),
            (Day::Tue, "18:30-19:45".to_string()),
            (Day::Wed, "18:30-19:45".to_string()),
        ]
    );
    assert!(
        snap.course("NOON")
            .unwrap()
            .meetings
            .iter()
            .all(|m| plain(&m.slot) == "11:50-13:05")
    );
}

/// Two branches can be given different columns. The master grid takes the
/// union, and every course keeps the times its own grid printed.
#[test]
fn branches_may_disagree_about_columns() {
    let out = Site::new("August--November 2027")
        .slots(&["09:10-10:25", "10:30-11:45", "11:50-13:05", "14:00-15:15"])
        .branch(
            "BM1",
            "B.S I year",
            &[
                ("Mon", &["AAA", "", "BBB", ""]),
                ("Tue", &["", "AAA", "", "BBB"]),
                ("Wed", &["AAA", "", "", "BBB"]),
            ],
        )
        .branch(
            "BM2",
            "B.S II year",
            &[
                ("Mon", &["", "CCC", "", "DDD"]),
                ("Wed", &["CCC", "", "DDD", ""]),
                ("Thu", &["", "CCC", "", "DDD"]),
            ],
        )
        .branch(
            "MC1",
            "M.Sc CS",
            &[
                ("Mon", &["EEE", "", "", ""]),
                ("Tue", &["", "EEE", "", ""]),
                ("Fri", &["", "", "EEE", ""]),
            ],
        )
        .course("AAA", "Course A", "T One")
        .course("BBB", "Course B", "T Two")
        .course("CCC", "Course C", "T Three")
        .course("DDD", "Course D", "T Four")
        .course("EEE", "Course E", "T Five")
        .reading_courses(6)
        .halls(&["Lecture Hall 1", "Lecture Hall 2", "Lecture Hall 3"])
        .hall_days(&["Monday", "Tuesday", "Wednesday", "Thursday", "Friday"])
        .book("Monday", "Lecture Hall 1", 0, "AAA")
        .book("Monday", "Lecture Hall 2", 1, "CCC")
        .book("Tuesday", "Lecture Hall 1", 1, "AAA")
        .book("Wednesday", "Lecture Hall 1", 0, "AAA")
        .read();
    let snap = snapshot_of(&out);
    assert_eq!(snap.slot_grid.len(), 4);
    // Every meeting's slot is one of the grid's columns.
    for course in &snap.courses {
        for m in &course.meetings {
            assert!(
                snap.slot_grid.contains(&m.slot),
                "{} meets at {} which is not a column of the master grid",
                course.code,
                m.slot.label()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Codes, names and legends
// ---------------------------------------------------------------------------

/// Codes are whatever CMI types. Case, digits, dots and hyphens all survive,
/// and a code is found however the reader capitalizes it.
#[test]
fn codes_are_taken_as_written() {
    let out = Site::new("August--November 2027")
        .slots(&["09:10-10:25", "10:30-11:45", "11:50-13:05", "14:00-15:15"])
        .branch(
            "BM1",
            "B.S I year",
            &[
                ("Mon", &["MA-101", "", "cs.2", ""]),
                ("Tue", &["", "PH2L", "", "X"]),
                ("Wed", &["MA-101", "", "LONGERCODE", ""]),
            ],
        )
        .branch(
            "BM2",
            "B.S II year",
            &[
                ("Mon", &["", "PH2L", "", "X"]),
                ("Wed", &["cs.2", "", "", "MA-101"]),
                ("Thu", &["", "LONGERCODE", "", ""]),
            ],
        )
        .branch(
            "MC1",
            "M.Sc CS",
            &[
                ("Tue", &["X", "", "", ""]),
                ("Wed", &["", "cs.2", "", ""]),
                ("Fri", &["", "", "PH2L", ""]),
            ],
        )
        .course("MA-101", "Analysis of a Kind", "A One")
        .course("cs.2", "Second Course in CS", "B Two")
        .course("PH2L", "Physics Lab II", "C Three")
        .course("X", "The Short One", "D Four")
        .course("LONGERCODE", "A Twelve Char Code", "E Five")
        .reading_courses(6)
        .halls(&["Lecture Hall 1", "Lecture Hall 2", "Lecture Hall 3"])
        .hall_days(&["Monday", "Tuesday", "Wednesday", "Thursday", "Friday"])
        .book("Monday", "Lecture Hall 1", 0, "MA-101")
        .book("Monday", "Lecture Hall 2", 2, "cs.2")
        .book("Tuesday", "Lecture Hall 1", 1, "PH2L")
        .book("Tuesday", "Lecture Hall 3", 3, "X")
        .read();
    let snap = snapshot_of(&out);

    for code in ["MA-101", "cs.2", "PH2L", "X", "LONGERCODE"] {
        assert!(
            snap.course(code).is_some(),
            "{code} was swallowed: {:?}",
            snap.courses.iter().map(|c| &c.code).collect::<Vec<_>>()
        );
    }
    // Lookup is case-insensitive both ways.
    assert_eq!(snap.course_ci("CS.2").unwrap().name, "Second Course in CS");
    assert_eq!(snap.course_ci("ma-101").unwrap().name, "Analysis of a Kind");
    assert_eq!(snap.course("X").unwrap().meetings.len(), 3);
}

/// The halls legend splits at the LAST colon, so a course name may contain
/// one. Names and instructors survive punctuation and non-ASCII text.
#[test]
fn names_may_contain_anything() {
    let out = january_term()
        .branch(
            "HUM",
            "Humanities & Electives",
            &[
                ("Mon", &["", "", "", "", "TPQI"]),
                ("Tue", &["", "", "", "", "FRE"]),
                ("Wed", &["", "", "", "", "SEMR"]),
            ],
        )
        .course("TPQI", "Topics: Quantum Information", "R Śrīnivāsan")
        .course("FRE", "French I & II", "Sumedha Turumella/A Visitor")
        .course("SEMR", "Seminar: Reading & Writing: Part II", "Speaker")
        .book("Monday", "Lecture Hall 2", 4, "TPQI")
        .book("Tuesday", "Lecture Hall 1", 4, "FRE")
        .book("Wednesday", "Lecture Hall 1", 4, "SEMR")
        .read();
    let snap = snapshot_of(&out);

    let t = snap.course("TPQI").unwrap();
    assert_eq!(t.name, "Topics: Quantum Information");
    assert_eq!(t.instructors, ["R Śrīnivāsan"]);

    let f = snap.course("FRE").unwrap();
    assert_eq!(f.name, "French I & II");
    assert_eq!(f.instructors, ["Sumedha Turumella", "A Visitor"]);

    // Two colons in the name: the last one still separates the instructor.
    assert_eq!(
        snap.course("SEMR").unwrap().name,
        "Seminar: Reading & Writing: Part II"
    );
}

/// Codes in the grid that no legend explains are the signal that the page
/// was published half-finished. A handful is normal; a flood is not.
#[test]
fn unexplained_codes_are_tolerated_then_refused() {
    // One unlisted code out of many: fine.
    let mostly_fine = january_term()
        .branch(
            "OM1",
            "Maths Electives",
            &[
                ("Mon", &["MYST", "", "", "", ""]),
                ("Wed", &["", "MYST", "", "", ""]),
                ("Fri", &["", "", "MYST", "", ""]),
            ],
        )
        .unlisted("MYST")
        .read();
    assert!(
        mostly_fine.report.gate_passed(),
        "one unexplained code must not sink the page: {:#?}",
        mostly_fine.report.gate
    );

    // A page where most codes explain nothing: refuse it.
    let mostly_broken = Site::new("August--November 2027")
        .slots(&["09:10-10:25", "10:30-11:45", "11:50-13:05", "14:00-15:15"])
        .branch(
            "BM1",
            "B.S I year",
            &[
                ("Mon", &["ZZ1", "", "ZZ2", ""]),
                ("Tue", &["", "ZZ3", "", "ZZ4"]),
                ("Wed", &["ZZ5", "", "ZZ6", ""]),
            ],
        )
        .branch(
            "BM2",
            "B.S II year",
            &[
                ("Mon", &["", "ZZ7", "", "ZZ8"]),
                ("Wed", &["ZZ9", "", "ZZA", ""]),
                ("Thu", &["", "ZZB", "", "ZZC"]),
            ],
        )
        .branch(
            "MC1",
            "M.Sc CS",
            &[
                ("Tue", &["ZZD", "", "", ""]),
                ("Wed", &["", "ZZE", "", ""]),
                ("Fri", &["", "", "ZZF", ""]),
            ],
        )
        .course("KNOWN", "The Only Explained Course", "A Teacher")
        .halls(&["Lecture Hall 1", "Lecture Hall 2", "Lecture Hall 3"])
        .hall_days(&["Monday", "Tuesday", "Wednesday"])
        .book("Monday", "Lecture Hall 1", 0, "ZZ1")
        .read();
    assert!(
        !mostly_broken.report.gate_passed(),
        "a page whose codes explain nothing must be refused"
    );
    assert!(
        failed_rules(&mostly_broken)
            .iter()
            .any(|r| r == "legend resolution"),
        "expected the legend-resolution rule to be the one that fired: {:?}",
        failed_rules(&mostly_broken)
    );
}

// ---------------------------------------------------------------------------
// The semester label
// ---------------------------------------------------------------------------

/// Two pages naming different terms is the stale-page signal: refuse, and
/// keep whatever was cached.
#[test]
fn pages_from_different_terms_are_refused() {
    let out = january_term()
        .halls_semester("August--November 2026")
        .read();
    assert!(!out.report.gate_passed());
    assert!(out.snapshot.is_none(), "fail closed: no half-snapshot");
    assert!(
        failed_rules(&out).iter().any(|r| r == "semester label"),
        "{:?}",
        failed_rules(&out)
    );
}

/// The same term phrased two ways on two independently edited pages is not
/// a conflict.
#[test]
fn the_same_term_phrased_two_ways_is_fine() {
    let out = january_term().halls_semester("Jan-Apr 2027").read();
    let snap = snapshot_of(&out);
    assert_eq!(snap.semester_label, "January--April 2027");
    assert!(
        out.report
            .warnings
            .iter()
            .any(|w| w.contains("phrased differently")),
        "the difference is worth a warning: {:?}",
        out.report.warnings
    );
}

/// A term with no label at all is still a term. The label is decoration;
/// the data is what matters.
#[test]
fn a_page_with_no_label_still_reads() {
    let out = Site::unlabelled()
        .slots(&["09:10-10:25", "10:30-11:45", "11:50-13:05", "14:00-15:15"])
        .branch(
            "BM1",
            "B.S I year",
            &[
                ("Mon", &["AAA", "", "BBB", ""]),
                ("Tue", &["", "AAA", "", "BBB"]),
                ("Wed", &["AAA", "", "CCC", ""]),
            ],
        )
        .branch(
            "BM2",
            "B.S II year",
            &[
                ("Mon", &["", "CCC", "", "DDD"]),
                ("Wed", &["CCC", "", "DDD", ""]),
                ("Thu", &["", "DDD", "", "EEE"]),
            ],
        )
        .branch(
            "MC1",
            "M.Sc CS",
            &[
                ("Tue", &["EEE", "", "", ""]),
                ("Wed", &["", "EEE", "", ""]),
                ("Fri", &["", "", "AAA", ""]),
            ],
        )
        .course("AAA", "Course A", "T One")
        .course("BBB", "Course B", "T Two")
        .course("CCC", "Course C", "T Three")
        .course("DDD", "Course D", "T Four")
        .course("EEE", "Course E", "T Five")
        .reading_courses(6)
        .halls(&["Lecture Hall 1", "Lecture Hall 2", "Lecture Hall 3"])
        .hall_days(&["Monday", "Tuesday", "Wednesday", "Thursday", "Friday"])
        .book("Monday", "Lecture Hall 1", 0, "AAA")
        .book("Tuesday", "Lecture Hall 2", 1, "AAA")
        .read();
    let snap = snapshot_of(&out);
    assert_eq!(snap.semester_label, "");
    assert_eq!(snap.courses.len(), 11);
    assert!(
        out.report
            .warnings
            .iter()
            .any(|w| w.contains("no semester label")),
        "{:?}",
        out.report.warnings
    );
}

// ---------------------------------------------------------------------------
// Broken pages
// ---------------------------------------------------------------------------

/// Everything that can arrive instead of a timetable, and the one thing
/// that must happen for all of it: no snapshot, no panic.
#[test]
fn nothing_broken_ever_gets_through() {
    let good = january_term();
    let tt = good.timetable_html();
    let halls = good.halls_html();

    let cases: Vec<(&str, String, String)> = vec![
        ("both pages empty", String::new(), String::new()),
        (
            "a 404 page",
            "<html><body><h1>404 Not Found</h1><p>The requested URL was not found.</p></body></html>".into(),
            "<html><body><h1>404 Not Found</h1></body></html>".into(),
        ),
        (
            "a PHP error",
            "<br /><b>Fatal error</b>:  Uncaught Error: Call to a member function on null in /var/www/timetable.php:31<br />".into(),
            halls.clone(),
        ),
        (
            "a login interstitial",
            "<html><body><form action=\"/login\"><input name=\"user\"><input name=\"pass\"></form></body></html>".into(),
            halls.clone(),
        ),
        (
            "the timetable truncated mid-grid",
            tt[..tt.len() / 3].to_string(),
            halls.clone(),
        ),
        (
            "the halls page truncated mid-grid",
            tt.clone(),
            halls[..halls.len() / 3].to_string(),
        ),
        ("the two pages swapped", halls.clone(), tt.clone()),
        ("the timetable served twice", tt.clone(), tt.clone()),
        (
            "a page with no <pre> at all",
            "<html><body><table><tr><td>BM1</td><td>09:10-10:25</td></tr></table></body></html>".into(),
            halls.clone(),
        ),
        ("nothing but whitespace", "   \n\n\t  ".into(), "\n".into()),
    ];

    for (what, tt_html, halls_html) in cases {
        let out = parse_html_pages(&tt_html, &halls_html, 0.0, SourceTier::Bundled, false);
        assert!(
            !out.report.gate_passed() && out.snapshot.is_none(),
            "{what}: this must never replace good data — gate {:#?}",
            out.report.gate
        );
        assert!(
            !out.report.errors.is_empty(),
            "{what}: a refusal has to say why"
        );
    }
}

/// Too little of a page to be a semester. Each of these is refused by the
/// rule that exists for it.
#[test]
fn the_floors_do_what_they_say() {
    let two_branches = Site::new("January--April 2027")
        .slots(&["09:10-10:25", "10:30-11:45", "11:50-13:05", "14:00-15:15"])
        .branch(
            "BM1",
            "B.S I year",
            &[
                ("Mon", &["AAA", "", "BBB", ""]),
                ("Tue", &["", "AAA", "", "BBB"]),
                ("Wed", &["AAA", "", "BBB", ""]),
            ],
        )
        .branch(
            "BM2",
            "B.S II year",
            &[
                ("Mon", &["", "CCC", "", "DDD"]),
                ("Wed", &["CCC", "", "DDD", ""]),
                ("Thu", &["", "CCC", "", "DDD"]),
            ],
        )
        .course("AAA", "Course A", "T One")
        .course("BBB", "Course B", "T Two")
        .course("CCC", "Course C", "T Three")
        .course("DDD", "Course D", "T Four")
        .halls(&["Lecture Hall 1", "Lecture Hall 2", "Lecture Hall 3"])
        .hall_days(&["Monday", "Tuesday", "Wednesday"])
        .book("Monday", "Lecture Hall 1", 0, "AAA")
        .read();
    assert!(
        failed_rules(&two_branches)
            .iter()
            .any(|r| r == "branch grid count"),
        "{:?}",
        failed_rules(&two_branches)
    );

    // A branch grid with two day rows is a grid that did not finish printing.
    let thin_grid = january_term()
        .branch(
            "OM2",
            "Maths Electives II",
            &[
                ("Mon", &["CAL1", "", "", "", ""]),
                ("Wed", &["", "CAL1", "", "", ""]),
            ],
        )
        .read();
    assert!(
        failed_rules(&thin_grid)
            .iter()
            .any(|r| r == "branch grid substance"),
        "{:?}",
        failed_rules(&thin_grid)
    );

    // One hall, one day: the halls page did not finish printing either.
    let thin_halls = january_term()
        .halls(&["Lecture Hall 1"])
        .hall_days(&["Monday"])
        .read();
    assert!(
        failed_rules(&thin_halls).iter().any(|r| r == "hall grid"),
        "{:?}",
        failed_rules(&thin_halls)
    );
}

/// The timetable page arrives truncated while the halls page is whole — the
/// shape no count floor can catch, because both pages parse to something.
#[test]
fn a_schedule_with_no_branches_behind_it_is_refused() {
    let out = Site::new("January--April 2027")
        .slots(&["09:10-10:25", "10:30-11:45", "11:50-13:05", "14:00-15:15"])
        // Three grids that between them schedule one course …
        .branch(
            "BM1",
            "B.S I year",
            &[
                ("Mon", &["AAA", "", "", ""]),
                ("Tue", &["", "AAA", "", ""]),
                ("Wed", &["AAA", "", "", ""]),
            ],
        )
        .branch(
            "BM2",
            "B.S II year",
            &[
                ("Mon", &["", "AAA", "", ""]),
                ("Wed", &["AAA", "", "", ""]),
                ("Thu", &["", "AAA", "", ""]),
            ],
        )
        .branch(
            "MC1",
            "M.Sc CS",
            &[
                ("Tue", &["AAA", "", "", ""]),
                ("Wed", &["", "AAA", "", ""]),
                ("Fri", &["", "", "AAA", ""]),
            ],
        )
        .course("AAA", "The One Branch Course", "T One")
        .course("BB1", "Hall Only One", "T Two")
        .course("BB2", "Hall Only Two", "T Three")
        .course("BB3", "Hall Only Three", "T Four")
        .course("BB4", "Hall Only Four", "T Five")
        .course("BB5", "Hall Only Five", "T Six")
        .course("BB6", "Hall Only Six", "T Seven")
        .course("BB7", "Hall Only Seven", "T Eight")
        .course("BB8", "Hall Only Eight", "T Nine")
        .course("BB9", "Hall Only Nine", "T Ten")
        // … while the halls page schedules nine more that no grid mentions.
        .halls(&["Lecture Hall 1", "Lecture Hall 2", "Lecture Hall 3"])
        .hall_days(&["Monday", "Tuesday", "Wednesday"])
        .book("Monday", "Lecture Hall 1", 0, "AAA")
        .book("Monday", "Lecture Hall 2", 1, "BB1")
        .book("Monday", "Lecture Hall 3", 2, "BB2")
        .book("Tuesday", "Lecture Hall 1", 0, "BB3")
        .book("Tuesday", "Lecture Hall 2", 1, "BB4")
        .book("Tuesday", "Lecture Hall 3", 2, "BB5")
        .book("Wednesday", "Lecture Hall 1", 0, "BB6")
        .book("Wednesday", "Lecture Hall 2", 1, "BB7")
        .book("Wednesday", "Lecture Hall 3", 2, "BB8")
        .book("Wednesday", "Lecture Hall 3", 3, "BB9")
        .read();
    assert!(
        failed_rules(&out)
            .iter()
            .any(|r| r == "cross-page consistency"),
        "{:?}",
        failed_rules(&out)
    );
    assert!(out.snapshot.is_none());
}

/// A page re-typed without its vertical rules is still readable — the
/// columns are still there in the spacing.
#[test]
fn a_hall_grid_without_pipes_still_reads() {
    let out = january_term().pipeless_halls().read();
    let snap = snapshot_of(&out);
    assert!(snap.halls.len() >= 6, "halls: {:?}", snap.halls);
    assert_eq!(
        meeting_days(snap, "CAL1"),
        [
            (
                Day::Mon,
                "08:30-09:45".into(),
                Some("Lecture Hall 1".into())
            ),
            (
                Day::Tue,
                "10:00-11:15".into(),
                Some("Lecture Hall 1".into())
            ),
            (
                Day::Wed,
                "08:30-09:45".into(),
                Some("Lecture Hall 1".into())
            ),
            (
                Day::Thu,
                "14:30-15:45".into(),
                Some("Lecture Hall 1".into())
            ),
        ],
        "every booking keeps its hall and its column without the rules"
    );
}

/// Whatever else changes, a course's meetings and the hall bookings that
/// describe them have to agree.
#[test]
fn the_two_pages_always_agree() {
    let out = january_term().read();
    let snap = snapshot_of(&out);
    for booking in &snap.hall_bookings {
        for code in &booking.codes {
            let Some(course) = snap.course_ci(code) else {
                continue; // an unexplained code has no course to check
            };
            assert!(
                course.meetings.iter().any(|m| m.day == booking.day
                    && m.slot == booking.slot
                    && m.hall.as_deref() == Some(booking.hall.as_str())),
                "{} is booked into {} on {} at {} but has no such meeting: {:?}",
                code,
                booking.hall,
                booking.day.full(),
                booking.slot.label(),
                course.meetings
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What the new term does to a planner that is already full
// ---------------------------------------------------------------------------

/// A student who has moved a class, struck one out, corrected a credit
/// count, deleted a course and written two of their own, on the morning CMI
/// publishes next semester. Nothing they did may be lost or silently
/// reinterpreted.
#[test]
fn a_new_term_meets_a_full_planner() {
    use cmi_timetable_core::merge::merge_overrides;
    use cmi_timetable_core::model::{Meeting, OverridesStore};

    let jan = january_term().read();
    let old = snapshot_of(&jan).clone();
    let aug = the_term_after().read();
    let new = snapshot_of(&aug).clone();

    // The new page really is a new term, read on its own terms.
    assert_eq!(new.semester_label, "August--November 2027");
    assert!(new.course("TOPO").is_none(), "BM2's courses went with it");
    assert!(new.course("DSCI").is_some(), "and a new one arrived");

    let pick = |snap: &Snapshot, code: &str, day: Day| -> Meeting {
        snap.course(code)
            .unwrap()
            .meetings
            .iter()
            .find(|m| m.day == day)
            .unwrap_or_else(|| panic!("{code} has no {day:?} meeting"))
            .clone()
    };

    let cal1_mon = pick(&old, "CAL1", Day::Mon);
    let topo_tue = pick(&old, "TOPO", Day::Tue);
    let cryp_tue = pick(&old, "CRYP", Day::Tue);

    let mut mine = OverridesStore::default();
    // Monday's Calculus is easier to get to from the other hall.
    let moved = Meeting {
        hall: Some("Seminar Hall".to_string()),
        ..cal1_mon
    };
    mine.add("CAL1", Some(cal1_mon), Some(moved), 1.0);
    // Tuesday's Topology clashes with something; strike it out.
    mine.add("TOPO", Some(topo_tue), None, 2.0);
    // Cryptography is worth 2 credits here, whatever the page implies.
    mine.set_credits("CRYP", 2, 3.0);
    // Quantum Field Theory is not for them.
    mine.hide("QFT", 4.0);

    let selection: Vec<String> = ["CAL1", "TOPO", "CRYP", "AIML"]
        .iter()
        .map(|s| s.to_string())
        .collect();

    let result = merge_overrides(&old, &new, &selection, &mine);

    // A selected course that no longer exists is reported, not deleted
    // behind the student's back.
    assert_eq!(result.removed_selected, ["TOPO"]);

    // CMI moved the class the student had moved: that is a question for
    // them, never an automatic answer.
    assert_eq!(result.conflicts.len(), 1, "{:#?}", result.conflicts);
    let conflict = &result.conflicts[0];
    assert_eq!(conflict.course, "CAL1");
    assert_eq!(
        conflict.mine.as_ref().unwrap().hall.as_deref(),
        Some("Seminar Hall")
    );
    assert_eq!(conflict.theirs.len(), 1);
    assert_eq!(plain(&conflict.theirs[0].slot), "10:00-11:15");
    assert_eq!(conflict.theirs[0].hall.as_deref(), Some("Lecture Hall 2"));

    // The struck-out meeting of a course CMI dropped is kept: if CMI puts
    // the course back next week, the student's decision is still there.
    assert!(
        result.overrides.for_course("TOPO").count() == 1,
        "a vanished course's overrides are kept, not swept up"
    );
    // Their credit correction and their deletion are none of the merge's
    // business, and it leaves both alone.
    assert_eq!(result.overrides.credits_for("CRYP"), Some(2));
    assert!(result.overrides.is_hidden("QFT"));
    assert!(result.dropped_matching.is_empty());

    // And the digest says what actually changed.
    assert!(result.diff.added.contains(&"DSCI".to_string()));
    assert!(result.diff.removed.contains(&"TOPO".to_string()));
    assert!(
        result.diff.changed.iter().any(|c| c.code == "CAL1"),
        "changed: {:?}",
        result
            .diff
            .changed
            .iter()
            .map(|c| &c.code)
            .collect::<Vec<_>>()
    );
    // A course nobody touched is not reported as changed.
    assert!(
        !result.diff.changed.iter().any(|c| c.code == "CRYP"),
        "CRYP did not move"
    );
    assert_eq!(cryp_tue, pick(&new, "CRYP", Day::Tue));
}

/// When CMI's new time turns out to be the time the student had already
/// written in, the disagreement is over — quietly.
#[test]
fn cmi_catching_up_with_the_student_is_not_a_conflict() {
    use cmi_timetable_core::merge::merge_overrides;
    use cmi_timetable_core::model::OverridesStore;

    let jan = january_term().read();
    let old = snapshot_of(&jan).clone();
    let aug = the_term_after().read();
    let new = snapshot_of(&aug).clone();

    let cal1_mon_old = old
        .course("CAL1")
        .unwrap()
        .meetings
        .iter()
        .find(|m| m.day == Day::Mon)
        .unwrap()
        .clone();
    let cal1_mon_new = new
        .course("CAL1")
        .unwrap()
        .meetings
        .iter()
        .find(|m| m.day == Day::Mon)
        .unwrap()
        .clone();

    let mut mine = OverridesStore::default();
    // The student had already heard the class was moving, and moved it.
    mine.add("CAL1", Some(cal1_mon_old), Some(cal1_mon_new), 1.0);

    let result = merge_overrides(&old, &new, &["CAL1".to_string()], &mine);
    assert!(result.conflicts.is_empty(), "{:#?}", result.conflicts);
    assert_eq!(result.dropped_matching.len(), 1);
    assert!(
        result.overrides.items.is_empty(),
        "the override has nothing left to say"
    );
}

/// The calendar a January term exports is a January calendar.
#[test]
fn the_exported_calendar_follows_the_term() {
    use cmi_timetable_core::date::semester_range_from_label;
    use cmi_timetable_core::ics::{IcsCourse, IcsOptions, build_ics, ics_filename};

    let out = january_term().read();
    let snap = snapshot_of(&out);

    let (start, end) = semester_range_from_label(&snap.semester_label)
        .expect("a term with months and a year has a range");
    assert_eq!(start.to_iso(), "2027-01-01");
    assert_eq!(end.to_iso(), "2027-04-30");

    let courses: Vec<IcsCourse> = ["CAL1", "CRYP", "RDNG"]
        .iter()
        .map(|code| {
            let c = snap.course(code).unwrap();
            IcsCourse::from_course(c, c.meetings.clone())
        })
        .collect();
    let ics = build_ics(
        &courses,
        &IcsOptions {
            range_start: start,
            range_end: end,
            alarm: false,
            app_url: "https://example.invalid/".to_string(),
            dtstamp: "20270101T000000Z".to_string(),
            calendar_name: snap.semester_label_display(),
        },
    );

    assert!(
        ics.contains("DTSTART;TZID=Asia/Kolkata:20270104T083000"),
        "{ics}"
    );
    assert!(ics.contains("RRULE:FREQ=WEEKLY;UNTIL=20270430T182959Z"));
    assert!(
        !ics.contains("2026"),
        "last year's dates must not survive into this term's calendar"
    );
    // A course with no meetings contributes no events.
    assert!(!ics.contains("RDNG"));
    // Every event is inside the term. (The VTIMEZONE block's own DTSTART is
    // the 1970 epoch and carries no TZID — it is not an event.)
    for line in ics.lines().filter(|l| l.starts_with("DTSTART;TZID=")) {
        let date = &line[line.len() - 15..line.len() - 7];
        assert!(
            ("20270101"..="20270430").contains(&date),
            "{line} falls outside January–April 2027"
        );
    }
    assert_eq!(
        ics_filename(&snap.semester_label),
        "cmi-timetable-jan-apr-2027.ics"
    );
}

/// A link written this term still says exactly what it said, courses of the
/// student's own and deleted courses included.
#[test]
fn a_link_carries_the_whole_planner() {
    use cmi_timetable_core::model::{Course, Meeting, OverridesStore, Slot};
    use cmi_timetable_core::share::{decode_share, encode_share, resolve_url_state};

    let out = january_term().read();
    let snap = snapshot_of(&out);

    let cal1_mon = snap
        .course("CAL1")
        .unwrap()
        .meetings
        .iter()
        .find(|m| m.day == Day::Mon)
        .unwrap()
        .clone();

    let mut mine = OverridesStore::default();
    mine.add(
        "CAL1",
        Some(cal1_mon.clone()),
        Some(Meeting {
            hall: Some("Seminar Hall".to_string()),
            ..cal1_mon
        }),
        1.0,
    );
    mine.set_credits("CRYP", 2, 2.0);
    mine.hide("QFT", 3.0);

    let reading_group = Course::custom(
        "RGRP".to_string(),
        "Reading group (Ramanujan)".to_string(),
        vec!["Us".to_string()],
        0,
        vec![Meeting {
            day: Day::Sat,
            slot: Slot::new(17 * 60, 18 * 60 + 30),
            hall: Some("Seminar Hall".to_string()),
            temp_booking: false,
        }],
    );
    let gym = Course::custom(
        "GYM".to_string(),
        "Gym".to_string(),
        vec![],
        0,
        vec![Meeting {
            day: Day::Wed,
            slot: Slot::new(19 * 60, 20 * 60),
            hall: None,
            temp_booking: false,
        }],
    );
    let customs = vec![reading_group, gym];

    let selection: Vec<String> = ["CAL1", "CRYP", "RGRP", "GYM"]
        .iter()
        .map(|s| s.to_string())
        .collect();

    let encoded = encode_share(&selection, &mine, &customs);
    let payload = decode_share(&encoded).expect("the link decodes");
    assert_eq!(payload.c, selection);
    assert_eq!(payload.o, mine.items);
    assert_eq!(payload.k, mine.credits);
    assert_eq!(payload.d, mine.hidden);
    assert_eq!(payload.x, customs);

    let state = resolve_url_state(None, Some(&encoded));
    let store = state.overrides.expect("the link carries changes");
    assert_eq!(store.credits_for("cryp"), Some(2));
    assert!(store.is_hidden("qft"));
    assert_eq!(state.customs[0].name, "Reading group (Ramanujan)");
    // The custom course's Saturday class survives, hall and all.
    assert_eq!(state.customs[0].meetings[0].day, Day::Sat);
    assert_eq!(
        state.customs[0].meetings[0].hall.as_deref(),
        Some("Seminar Hall")
    );
}

/// A term that crosses New Year is written with both years. The whole label
/// has to survive the heading — a label truncated to "December 2026" would
/// be contradicted by a halls page that spells the term out, refusing a
/// perfectly good page, and would leave the calendar with no term to export
/// into.
#[test]
fn a_term_that_crosses_new_year_survives_whole() {
    use cmi_timetable_core::date::semester_range_from_label;

    let out = january_term()
        .relabel("December 2026--March 2027")
        .halls_semester("Dec 2026 - Mar 2027")
        .read();
    let snap = snapshot_of(&out);

    assert_eq!(snap.semester_label, "December 2026--March 2027");
    assert_eq!(
        snap.semester_label_display(),
        "December 2026\u{2013}March 2027"
    );
    assert!(
        out.report
            .warnings
            .iter()
            .any(|w| w.contains("phrased differently")),
        "the two spellings name one term: {:?}",
        out.report.warnings
    );

    let (start, end) = semester_range_from_label(&snap.semester_label)
        .expect("a term that names its months and years has a range");
    assert_eq!(start.to_iso(), "2026-12-01");
    assert_eq!(end.to_iso(), "2027-03-31");
}

/// Two branch grids can end the same hour at different minutes — a lab
/// running fifteen minutes over, say. That is one column described twice,
/// not two columns: everything downstream places a class by the minute it
/// starts, so a grid holding both would draw every class in that hour once
/// per column, and every hall booking twice with it.
#[test]
fn two_grids_ending_an_hour_differently_still_make_one_column() {
    let out = january_term()
        .branch(
            "LAB1",
            "Physics Lab group",
            &[
                ("Mon", &["", "", "", "LABW", ""]),
                ("Wed", &["", "", "", "LABW", ""]),
                ("Fri", &["", "", "", "LABW", ""]),
            ],
        )
        // Same five columns, except the afternoon one runs on.
        .own_columns(&[
            "08:30-09:45",
            "10:00-11:15",
            "11:30-12:45",
            "14:30-16:00",
            "16:00-17:15",
        ])
        .read();
    let snap = snapshot_of(&out);

    let starts: Vec<u16> = snap.slot_grid.iter().map(|s| s.start_min).collect();
    let mut unique = starts.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        starts.len(),
        unique.len(),
        "no two columns may start at the same minute: {:?}",
        snap.slot_grid.iter().map(plain).collect::<Vec<_>>()
    );
    assert_eq!(
        snap.slot_grid.iter().map(plain).collect::<Vec<_>>(),
        [
            "08:30-09:45",
            "10:00-11:15",
            "11:30-12:45",
            "14:30-15:45",
            "16:00-17:15"
        ],
        "the first grid's reading of the column is the one kept"
    );
    assert!(
        out.report.warnings.iter().any(|w| w.contains("instead of")),
        "and the disagreement is worth saying out loud: {:?}",
        out.report.warnings
    );

    // The lab class still knows its own real times.
    let lab = snap.course("LABW").unwrap();
    assert!(
        lab.meetings.iter().any(|m| plain(&m.slot) == "14:30-16:00"),
        "{:?}",
        lab.meetings
    );
}

// ---------------------------------------------------------------------------
// The five ways the parser used to lie quietly (CONTEXT.md §8)
//
// Every one of these produced a page that LOOKED fine: the gate passed, the
// counts were plausible, and the student had no way to tell. That is what
// makes them worth a test each.
// ---------------------------------------------------------------------------

/// §8.1 — a day line CMI reworded is still that day.
///
/// The strict day reader refuses "Thursday - 6 Nov", and has to: it also
/// reads the rows that CARRY CLASSES, where "Mon-Fri" must never become
/// Monday. The hall grid's day lines carry nothing, so refusing them there
/// bought nothing and cost a whole day — every hall row beneath was filed
/// under Wednesday, silently.
#[test]
fn a_day_line_with_a_date_after_it_is_still_that_day() {
    let out = january_term()
        .retype_halls("Thursday", "Thursday - 6 Nov")
        .read();
    let snap = snapshot_of(&out);

    // CRYP meets Thursday 11:30 in the Seminar Hall. If the day line went
    // unread, this booking lands on Wednesday instead.
    assert_eq!(
        meeting_days(snap, "CRYP")
            .into_iter()
            .filter(|(d, _, _)| *d == Day::Thu)
            .count(),
        1,
        "Thursday's bookings must stay on Thursday: {:?}",
        meeting_days(snap, "CRYP")
    );
    assert!(
        snap.hall_bookings.iter().any(|b| b.day == Day::Thu),
        "the hall grid must still have a Thursday"
    );
    assert!(
        out.report
            .warnings
            .iter()
            .any(|w| w.contains("not a plain day name")),
        "and reading it loosely is worth saying out loud: {:?}",
        out.report.warnings
    );
}

/// §8.1 — a day line it CANNOT read is refused, not merged.
///
/// A typo has no reading. The old code filed the rows beneath it under the
/// previous day and every count it produced stayed plausible, so nothing
/// noticed. The structural fact does: a hall cannot have two rows in one
/// day.
#[test]
fn a_misspelled_day_line_fails_the_gate_instead_of_merging_two_days() {
    let out = january_term().retype_halls("Thursday", "Thrusday").read();

    assert!(
        out.snapshot.is_none(),
        "a merged day must not reach the student"
    );
    assert!(
        failed_rules(&out)
            .iter()
            .any(|r| r == "hall grid day sections"),
        "and the rule that catches it must be the one that names the problem: {:?}",
        failed_rules(&out)
    );
}

/// §8.2 — a legend belongs to the grid above it, or to nobody.
///
/// When a branch grid's day rows cannot be read, no section is opened for
/// it — and the legend underneath used to be appended to `sections.last()`,
/// i.e. the PREVIOUS branch, which then listed courses it does not teach.
///
/// The branch mangled here is deliberately a SMALL one whose courses the
/// hall grid never books. Lose a big grid and the cross-page rule already
/// refuses the page; lose a small one and everything still passes the gate,
/// which is precisely when a wrong branch reaches a student.
#[test]
fn a_legend_is_never_credited_to_the_branch_above_the_one_it_belongs_to() {
    let with_reading_group = |site: Site| {
        site.course("RGA", "Reading Group A", "Various")
            .course("RGB", "Reading Group B", "Various")
            .branch(
                "RG1",
                "Reading groups",
                &[
                    ("MON.", &["RGA", "", "", "", ""]),
                    ("WED.", &["", "", "RGB", "", ""]),
                    ("FRI.", &["", "RGA", "", "", ""]),
                ],
            )
    };
    // A control first: read as printed, the reading group is its own branch
    // and OP1 is untouched.
    let control = snapshot_of(&with_reading_group(january_term()).read()).clone();
    assert_eq!(
        control.course("RGA").unwrap().branches,
        vec!["RG1".to_string()],
        "control: RGA belongs to RG1"
    );

    // Now RG1's day labels are unreadable. "MON." is unique to this grid —
    // every other branch prints "Mon" or "Monday".
    let out = with_reading_group(january_term())
        .retype_timetable("MON.", "MOM.")
        .retype_timetable("WED.", "WEB.")
        .retype_timetable("FRI.", "FRO.")
        .read();
    let snap = snapshot_of(&out);

    assert!(
        snap.branch("RG1").is_none(),
        "a grid with no readable day rows yields no branch"
    );
    for code in ["RGA", "RGB"] {
        let course = snap.course(code).unwrap_or_else(|| panic!("{code} lost"));
        assert!(
            course.branches.is_empty(),
            "{code} is RG1's, and RG1's grid was unreadable — it must not be \
             handed to the branch printed above it: {:?}",
            course.branches
        );
    }
    // The courses themselves survive: we lost who teaches them, not them.
    assert_eq!(snap.course("RGA").unwrap().name, "Reading Group A");
    assert!(
        out.report
            .warnings
            .iter()
            .any(|w| w.contains("which branch these")),
        "{:?}",
        out.report.warnings
    );
}

/// §8.3 — a row with one separator too many keeps its hall's name.
///
/// The header's label cell is a character narrower than the rows' on the
/// live page. When a row's separator count differs, it is sliced at the
/// header's positions instead — and the cut landed one character inside the
/// longest hall names, shearing them and leaving the row's own separator at
/// the end of the cell, which invented rooms like "Lecture Hall 20".
#[test]
fn a_hall_row_with_a_stray_separator_keeps_its_name() {
    let base = january_term();
    let html = base.halls_html();
    // Monday's row for the longest-named hall, verbatim, plus one separator.
    let row = html
        .lines()
        .find(|l| l.contains("Lecture Hall 205"))
        .expect("the hall grid has a Lecture Hall 205 row")
        .to_string();
    let out = base.retype_halls(&row, &format!("{row}|")).read();
    let snap = snapshot_of(&out);

    assert!(
        snap.halls.iter().any(|h| h == "Lecture Hall 205"),
        "the hall keeps its name: {:?}",
        snap.halls
    );
    assert!(
        !snap
            .halls
            .iter()
            .any(|h| h != "Lecture Hall 205" && h.starts_with("Lecture Hall 20")),
        "and no sheared copy of it appears: {:?}",
        snap.halls
    );
    assert!(
        snap.halls.iter().all(|h| !h.contains('|')),
        "no hall name may contain a separator: {:?}",
        snap.halls
    );
    // The booking in that row is still readable.
    assert!(
        snap.hall_bookings
            .iter()
            .any(|b| b.hall == "Lecture Hall 205" && b.day == Day::Mon),
        "Monday's booking survives the repair"
    );
}

/// §8.4 — the same course typed in two cases is one course.
///
/// The pages are edited by hand and independently. Keying on the text as
/// printed made `CAL1` and `Cal1` two courses: one holding the classes with
/// no room, one holding the room with no classes.
#[test]
fn a_code_cased_differently_on_the_two_pages_is_still_one_course() {
    let out = january_term().retype_halls("CAL1", "Cal1").read();
    let snap = snapshot_of(&out);

    let matches: Vec<&str> = snap
        .courses
        .iter()
        .filter(|c| c.code.eq_ignore_ascii_case("CAL1"))
        .map(|c| c.code.as_str())
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "one course, not one per spelling: {matches:?}"
    );

    let course = snap.course_ci("cal1").expect("found however it is cased");
    assert_eq!(course.name, "Calculus I");
    assert!(
        !course.meetings.is_empty() && course.meetings.iter().all(|m| m.hall.is_some()),
        "and it keeps the rooms the halls page gave it: {:?}",
        course.meetings
    );
}

/// §8.5 — a note on one page is not lost because the other page is terser.
///
/// The halls legend wins the NAME, because it is the catalog. It used to
/// win the notes with it: if the timetable legend said "(starts 12 Feb)"
/// and the halls legend did not, the course silently became a normal
/// full-semester one — in the planner, in the credit total and in the
/// exported calendar.
#[test]
fn a_course_note_survives_a_terser_name_on_the_other_page() {
    let out = january_term()
        .retype_timetable("Combinatorics", "Combinatorics (starts 12 Feb)")
        .retype_timetable("Topology", "Topology (Oct-Nov)")
        .retype_timetable("General Relativity", "General Relativity (2 credits)")
        .read();
    let snap = snapshot_of(&out);

    let comb = snap.course("COMB").unwrap();
    assert_eq!(comb.name, "Combinatorics", "the catalog's name is shown");
    assert_eq!(
        comb.starts,
        Some((12, "Feb".to_string())),
        "but the date it starts is not thrown away with the wording"
    );

    assert_eq!(
        snap.course("TOPO").unwrap().part_of_semester,
        Some("Oct-Nov".to_string())
    );
    assert_eq!(snap.course("GRAV").unwrap().credits, Some(2));

    // And the same course read from a page that says nothing extra is
    // unchanged — the union may only ADD what the other page knew.
    let plain_read = january_term().read();
    let plain_snap = snapshot_of(&plain_read);
    assert_eq!(plain_snap.course("COMB").unwrap().starts, None);
}
