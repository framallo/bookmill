//! Ratatui terminal UI: browse books and launch builds per language / edition.
//! On Enter it returns a BuildReq; main runs the build after the TUI tears down
//! (so build output streams normally instead of fighting the alt-screen).

use crate::build::{self, Job, QueueEvent};
use crate::discover::Repo;
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Gauge, List, ListItem, ListState, Paragraph, Wrap},
};
use std::collections::BTreeMap;
use std::io::stdout;
use std::sync::mpsc;
use std::time::Duration;

pub struct BuildReq {
    pub book: String,
    pub lang: String,
    pub edition: String,
    /// "edition" => build by the selected edition; otherwise a format (epub|pdf|kdp|print|all)
    pub format: String,
}

/// What the selector screen returns when the user acts.
pub enum Action {
    /// quick build of the selected format/edition (no validations)
    Build(BuildReq),
    /// complete build: all editions + covers + deep validate
    Release(BuildReq),
    /// deep validation (epubcheck + geometry + cover)
    Validate { book: String },
    /// render covers for `book` (lang = "all" or a single language)
    Covers { book: String, lang: String },
    /// render audiobook
    Audiobook { book: String, lang: String },
}

/// The focusable column-lists on the selector screen, left → right in tab order.
/// ←/→ move focus between them; ↑/↓ move the selection within the focused one.
#[derive(Clone, Copy, PartialEq)]
enum Pane {
    Books,
    Action,
    Format,
    Language,
    Edition,
}

impl Pane {
    /// Focus order for ←/→ (wraps around).
    const ORDER: [Pane; 5] = [Pane::Books, Pane::Action, Pane::Format, Pane::Language, Pane::Edition];
    fn idx(self) -> usize {
        Self::ORDER.iter().position(|&p| p == self).unwrap_or(0)
    }
    fn next(self) -> Pane {
        Self::ORDER[(self.idx() + 1) % Self::ORDER.len()]
    }
    fn prev(self) -> Pane {
        Self::ORDER[(self.idx() + Self::ORDER.len() - 1) % Self::ORDER.len()]
    }
}

/// Actions the TUI can run (display name, one-line description). "all" is first
/// (the default) so the action list, like the others, starts with the
/// build-everything choice; it runs the same pipeline as Release.
const ACTIONS: &[(&str, &str)] = &[
    ("all", "run everything: all editions + covers + deep validate"),
    ("Build", "quick build of the selected format/edition — no checks"),
    ("Release", "complete: all editions + covers + deep validate"),
    ("Validate", "deep checks: epubcheck + PDF geometry + cover"),
    ("Covers", "render front + wrap covers (resvg)"),
    ("Audiobook", "render audiobook (kab engine)"),
];

/// Format selector options; "all" first (the default) so the cursor starts on the
/// build-everything choice. "edition" means use the edition selector instead.
const FORMATS: &[&str] = &["all", "edition", "epub", "pdf", "kdp", "print"];

/// Single source of truth for the copy-pasteable command a selection maps to.
/// Shared by the live TUI preview (draw loop) and `Action::command()` (printed by
/// main just before the action runs), so what you see is what runs.
fn cmd_string(action: &str, slug: &str, lang: &str, edition: &str, format: &str) -> String {
    let langflag = if lang != "all" {
        format!(" --lang {lang}")
    } else {
        String::new()
    };
    match action {
        "Validate" => format!("bookmill validate {slug} --deep"),
        "Covers" => format!("bookmill build cover {slug}{langflag}"),
        "Audiobook" => format!("bookmill audiobook {slug}{langflag}"),
        "Release" | "all" => format!("bookmill build {slug}{langflag} ; build cover ; validate --deep"),
        _ => {
            let mut c = format!("bookmill build {slug}");
            if format == "edition" {
                if edition != "all" {
                    c.push_str(&format!(" --edition {edition}"));
                }
            } else {
                c.push_str(&format!(" --format {format}"));
            }
            c.push_str(&langflag);
            c
        }
    }
}

impl Action {
    /// A clear, copy-pasteable representation of the command this action runs —
    /// printed by main just before execution so the user sees exactly what runs.
    pub fn command(&self) -> String {
        match self {
            Action::Build(r) => cmd_string("Build", &r.book, &r.lang, &r.edition, &r.format),
            Action::Release(r) => cmd_string("Release", &r.book, &r.lang, &r.edition, &r.format),
            Action::Validate { book } => cmd_string("Validate", book, "all", "all", "edition"),
            Action::Covers { book, lang } => cmd_string("Covers", book, lang, "all", "edition"),
            Action::Audiobook { book, lang } => cmd_string("Audiobook", book, lang, "all", "edition"),
        }
    }
}

fn format_outputs(fmt: &str) -> &'static str {
    match fmt {
        "epub" => "EPUB (retail)",
        "pdf" => "PDF (retail, with cover)",
        "kdp" => "EPUB + PDF (KDP)",
        "print" => "PDF (KDP print interior)",
        "all" => "EPUB + PDF (retail + KDP)",
        _ => "",
    }
}

struct Book {
    slug: String,
    title: String,
    langs: Vec<String>,
    editions: Vec<String>,
}

/// Human description of what a target produces (mirrors build::outputs_for_target).
fn target_outputs(target: &str) -> &'static str {
    match target {
        "kdp-paperback" => "PDF — bleed print interior (KDP paperback)",
        "kdp-epub" => "EPUB — Kindle (lower-res images)",
        "kdp" => "EPUB + PDF (KDP)",
        "gumroad" => "EPUB + PDF — retail (direct digital)",
        "bubok" => "PDF — print interior (POD, per region)",
        _ => "EPUB + PDF — retail",
    }
}

pub fn run(repo: &Repo) -> Result<Option<Action>> {
    let mut books: Vec<Book> = Vec::new();
    for dir in repo.book_dirs()? {
        if let Ok((b, _)) = repo.load_book_at(&dir) {
            books.push(Book {
                title: b.title.values().next().cloned().unwrap_or_else(|| b.slug.clone()),
                slug: b.slug,
                langs: b.languages,
                editions: b.editions,
            });
        }
    }
    if books.is_empty() {
        println!("no books found (need a book-level bookmill.toml under the books dir)");
        return Ok(None);
    }
    // edition name -> target (for the outputs description)
    let ed_target: BTreeMap<String, String> = repo
        .config
        .editions
        .iter()
        .map(|(k, e)| (k.clone(), e.target.clone().unwrap_or_else(|| "retail".into())))
        .collect();

    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    let mut term = Terminal::new(CrosstermBackend::new(stdout()))?;
    let mut bsel = ListState::default();
    bsel.select(Some(0));
    let mut lang_i = 0usize; // 0 = all
    let mut ed_i = 0usize; // 0 = all
    let mut fmt_i = 0usize; // 0 = "all" (build every format — the default)
    let mut act_i = 0usize; // index into ACTIONS
    // Which column-list has keyboard focus. ↑/↓ move the selection inside it,
    // ←/→ jump focus between lists. Each list keeps its own cursor (the *_i vars),
    // so focus changes never reset a list's position.
    let mut focus = Pane::Books;
    let mut result: Option<Action> = None;

    let opts = |first: &str, rest: &[String]| {
        let mut v = vec![first.to_string()];
        v.extend(rest.iter().cloned());
        v
    };

    let r = (|| -> Result<()> {
        loop {
            let bi = bsel.selected().unwrap_or(0);
            let book = &books[bi];
            let lopts = opts("all", &book.langs);
            let eopts = opts("all", &book.editions);
            let lang = lopts[lang_i.min(lopts.len() - 1)].clone();
            let edition = eopts[ed_i.min(eopts.len() - 1)].clone();
            let format = FORMATS[fmt_i % FORMATS.len()].to_string();
            let by_edition = format == "edition";
            let action = ACTIONS[act_i % ACTIONS.len()];
            let is_build = matches!(action.0, "all" | "Build" | "Release");

            // command preview (action-aware) — same string main prints before running
            let cmd = cmd_string(action.0, &book.slug, &lang, &edition, &format);
            // outputs description
            let outputs = if !by_edition {
                format_outputs(&format).to_string()
            } else if edition == "all" {
                format!("all {} edition(s): {}", book.editions.len(), book.editions.join(", "))
            } else {
                let t = ed_target.get(&edition).map(String::as_str).unwrap_or("retail");
                target_outputs(t).to_string()
            };
            let langs_desc = if lang == "all" { book.langs.join(" + ") } else { lang.clone() };

            term.draw(|f| {
                let v = Layout::vertical([
                    Constraint::Length(1), // title
                    Constraint::Min(8),    // body
                    Constraint::Length(3), // help box
                ])
                .split(f.area());

                f.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled(" bookmill ", Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)),
                        Span::styled("  interactive build", Style::default().fg(Color::Gray)),
                    ])),
                    v[0],
                );

                // stacked: books list, then a row of full option lists, then the preview
                let rows = Layout::vertical([
                    Constraint::Min(5),    // books list
                    Constraint::Length(9), // option lists (action/format/language/edition)
                    Constraint::Length(8), // preview panel
                ])
                .split(v[1]);

                // A focused list gets a bright yellow border; unfocused stays gray.
                let border = |focused: bool| {
                    if focused {
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    }
                };
                let books_focused = focus == Pane::Books;

                // books list — full list of every book
                let items: Vec<ListItem> = books
                    .iter()
                    .map(|b| ListItem::new(format!("{}  ·  {}", b.slug, b.title)))
                    .collect();
                f.render_stateful_widget(
                    List::new(items)
                        .block(
                            Block::default()
                                .borders(Borders::ALL)
                                .border_style(border(books_focused))
                                .title(" Books "),
                        )
                        .highlight_symbol("▶ ")
                        .highlight_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD | Modifier::REVERSED)),
                    rows[0],
                    &mut bsel.clone(),
                );

                // Row of option lists: every action / format / language / edition, all
                // visible at once with the current pick highlighted.
                let cols = Layout::horizontal([
                    Constraint::Percentage(28), // Action
                    Constraint::Percentage(18), // Format
                    Constraint::Percentage(22), // Language
                    Constraint::Percentage(32), // Edition
                ])
                .split(rows[1]);

                // Reusable option-list renderer: draws all `items`, highlighting `sel`.
                // `focused` = has keyboard focus (bright border). `active` = relevant to
                // the current action; when false the list is dimmed (n/a, e.g. Format
                // when the action isn't a build).
                let mut opt_list =
                    |area: Rect, title: &str, items: &[String], sel: usize, active: bool, focused: bool| {
                        let hl = if active { Color::Cyan } else { Color::DarkGray };
                        let title_style = if focused {
                            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                        } else if active {
                            Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::DarkGray)
                        };
                        let li: Vec<ListItem> =
                            items.iter().map(|s| ListItem::new(s.clone())).collect();
                        let mut st = ListState::default();
                        if !items.is_empty() {
                            st.select(Some(sel.min(items.len() - 1)));
                        }
                        f.render_stateful_widget(
                            List::new(li)
                                .block(
                                    Block::default()
                                        .borders(Borders::ALL)
                                        .border_style(border(focused))
                                        .title(Span::styled(title.to_string(), title_style)),
                                )
                                .highlight_symbol("▶ ")
                                .highlight_style(
                                    Style::default().fg(hl).add_modifier(Modifier::BOLD | Modifier::REVERSED),
                                ),
                            area,
                            &mut st,
                        );
                    };

                let act_items: Vec<String> = ACTIONS.iter().map(|(n, _)| n.to_string()).collect();
                let fmt_items: Vec<String> = FORMATS.iter().map(|s| s.to_string()).collect();
                let lang_active = action.0 != "Validate";
                opt_list(cols[0], " Action ", &act_items, act_i % ACTIONS.len(), true, focus == Pane::Action);
                opt_list(cols[1], " Format ", &fmt_items, fmt_i % FORMATS.len(), is_build, focus == Pane::Format);
                opt_list(cols[2], " Language ", &lopts, lang_i.min(lopts.len() - 1), lang_active, focus == Pane::Language);
                opt_list(cols[3], " Edition ", &eopts, ed_i.min(eopts.len() - 1), is_build && by_edition, focus == Pane::Edition);

                // preview panel — resolved selection + the exact command that will run
                let lbl = |s: &'static str| Span::styled(s, Style::default().fg(Color::Gray));
                let lines = vec![
                    Line::from(vec![lbl("Action    "), Span::styled(format!("{} — {}", action.0, action.1), Style::default().fg(Color::Magenta))]),
                    Line::from(vec![lbl("Book      "), Span::styled(&book.slug, Style::default().fg(Color::White).add_modifier(Modifier::BOLD))]),
                    Line::from(vec![lbl(if is_build { "Builds    " } else { "Does      " }), Span::styled(if is_build { outputs } else { action.1.to_string() }, Style::default().fg(Color::Green))]),
                    Line::from(vec![lbl("Language  "), Span::styled(langs_desc, Style::default().fg(Color::Green))]),
                    Line::from(vec![lbl("Runs      "), Span::styled(cmd, Style::default().fg(Color::Yellow))]),
                    Line::raw(""),
                    Line::from(Span::styled(format!("  press ⏎ Enter to run: {}  ", action.0), Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD))),
                ];
                f.render_widget(
                    Paragraph::new(lines)
                        .wrap(Wrap { trim: true })
                        .block(Block::default().borders(Borders::ALL).title(" Selection ")),
                    rows[2],
                );

                // help box
                f.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled(" ←/→ ", Style::default().fg(Color::Black).bg(Color::Yellow)),
                        Span::raw(" focus list   "),
                        Span::styled(" ↑/↓ ", Style::default().fg(Color::Black).bg(Color::Gray)),
                        Span::raw(" move in list   "),
                        Span::styled(" a f l e ", Style::default().fg(Color::Black).bg(Color::Gray)),
                        Span::raw(" jump to action/format/lang/edition   "),
                        Span::styled(" Enter ", Style::default().fg(Color::Black).bg(Color::Green)),
                        Span::raw(" run   "),
                        Span::styled(" q ", Style::default().fg(Color::Black).bg(Color::Red)),
                        Span::raw(" quit"),
                    ]))
                    .block(Block::default().borders(Borders::ALL).title(" Controls ")),
                    v[2],
                );
            })?;

            if let Event::Key(k) = event::read()? {
                let nb = books.len();
                // ↑/↓ helpers: clamp within a list of length `len`.
                let dec = |i: usize| i.saturating_sub(1);
                let inc = |i: usize, len: usize| (i + 1).min(len.saturating_sub(1));
                match k.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    // ←/→ : move focus between lists — each keeps its own cursor.
                    KeyCode::Left => focus = focus.prev(),
                    KeyCode::Right => focus = focus.next(),
                    // ↑/↓ (or k/j): move the selection inside the focused list.
                    KeyCode::Up | KeyCode::Char('k') => match focus {
                        Pane::Books => bsel.select(Some(dec(bi))),
                        Pane::Action => act_i = dec(act_i),
                        Pane::Format => fmt_i = dec(fmt_i),
                        Pane::Language => lang_i = dec(lang_i),
                        Pane::Edition => ed_i = dec(ed_i),
                    },
                    KeyCode::Down | KeyCode::Char('j') => match focus {
                        Pane::Books => bsel.select(Some(inc(bi, nb))),
                        Pane::Action => act_i = inc(act_i, ACTIONS.len()),
                        Pane::Format => fmt_i = inc(fmt_i, FORMATS.len()),
                        Pane::Language => lang_i = inc(lang_i, lopts.len()),
                        Pane::Edition => ed_i = inc(ed_i, eopts.len()),
                    },
                    // Single-key jumps straight to a list (then ↑/↓ to change it).
                    KeyCode::Char('b') => focus = Pane::Books,
                    KeyCode::Char('a') => focus = Pane::Action,
                    KeyCode::Char('f') => focus = Pane::Format,
                    KeyCode::Char('l') => focus = Pane::Language,
                    KeyCode::Char('e') => focus = Pane::Edition,
                    KeyCode::Enter => {
                        let b = book.slug.clone();
                        result = Some(match action.0 {
                            "Release" | "all" => Action::Release(BuildReq { book: b, lang, edition, format }),
                            "Validate" => Action::Validate { book: b },
                            "Covers" => Action::Covers { book: b, lang },
                            "Audiobook" => Action::Audiobook { book: b, lang },
                            _ => Action::Build(BuildReq { book: b, lang, edition, format }),
                        });
                        break;
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    })();

    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    term.show_cursor()?;
    r?;
    Ok(result)
}

#[derive(Clone, Copy, PartialEq)]
enum JobStatus {
    Pending,
    Running,
    Ok,
    Fail,
}

/// Live queue progress screen: runs the build in a scoped thread and renders the
/// job list (✓ / ▶ / ·) with an ETA line + gauge. Returns Err if any job failed.
/// `cmd` is the command this queue represents, shown in the header so the user
/// sees what is running (the alt-screen hides anything printed to stdout before).
pub fn run_queue_ui(repo: &Repo, jobs: &[Job], cmd: &str) -> Result<()> {
    if jobs.is_empty() {
        println!("(no jobs to build)");
        return Ok(());
    }
    let labels: Vec<String> = jobs.iter().map(|j| j.label()).collect();
    let n = labels.len();

    let mut statuses = vec![JobStatus::Pending; n];
    let mut errs: Vec<Option<String>> = vec![None; n];
    let mut current = 0usize; // 1-based index of running job; 0 = none yet
    let mut elapsed = Duration::ZERO;
    let mut eta: Option<Duration> = None;
    let mut completed = 0usize;
    let mut failures = 0usize;
    let mut finished = false;

    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    let mut term = Terminal::new(CrosstermBackend::new(stdout()))?;

    let (tx, rx) = mpsc::channel::<QueueEvent>();
    let build_result = std::thread::scope(|s| -> Result<()> {
        let tx2 = tx.clone();
        let handle = s.spawn(move || {
            let mut cb = |e: QueueEvent| {
                let _ = tx2.send(e);
            };
            build::run_queue(repo, jobs, &mut cb)
        });

        loop {
            // drain progress events
            while let Ok(ev) = rx.try_recv() {
                match ev {
                    QueueEvent::Start {
                        idx,
                        elapsed: el,
                        eta: et,
                        ..
                    } => {
                        current = idx;
                        if idx >= 1 && idx <= n {
                            statuses[idx - 1] = JobStatus::Running;
                        }
                        elapsed = el;
                        eta = et;
                    }
                    QueueEvent::Done { idx, err, .. } => {
                        if idx >= 1 && idx <= n {
                            statuses[idx - 1] = if err.is_some() {
                                JobStatus::Fail
                            } else {
                                JobStatus::Ok
                            };
                            errs[idx - 1] = err;
                        }
                        completed += 1;
                    }
                    QueueEvent::Summary {
                        failures: f,
                        elapsed: el,
                        ..
                    } => {
                        failures = f;
                        elapsed = el;
                        finished = true;
                    }
                }
            }

            let ratio = (completed as f64 / n as f64).clamp(0.0, 1.0);
            term.draw(|f| {
                let v = Layout::vertical([
                    Constraint::Length(1), // title
                    Constraint::Length(3), // gauge
                    Constraint::Min(4),    // job list
                    Constraint::Length(3), // footer
                ])
                .split(f.area());

                f.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled(
                            " bookmill ",
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Yellow)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled("  running: ", Style::default().fg(Color::Gray)),
                        Span::styled(cmd, Style::default().fg(Color::Yellow)),
                    ])),
                    v[0],
                );

                let eta_s = match (finished, eta) {
                    (true, _) => "done".to_string(),
                    (false, Some(d)) => format!("~{} left", build::fmt_dur(d)),
                    (false, None) => "estimating…".to_string(),
                };
                f.render_widget(
                    Gauge::default()
                        .block(Block::default().borders(Borders::ALL).title(format!(
                            " {}/{} jobs · elapsed {} · {} ",
                            completed,
                            n,
                            build::fmt_dur(elapsed),
                            eta_s
                        )))
                        .gauge_style(Style::default().fg(Color::Green))
                        .ratio(ratio),
                    v[1],
                );

                let items: Vec<ListItem> = labels
                    .iter()
                    .enumerate()
                    .map(|(i, lbl)| {
                        let (mark, style) = match statuses[i] {
                            JobStatus::Ok => (
                                "\u{2713}",
                                Style::default().fg(Color::Green),
                            ),
                            JobStatus::Fail => (
                                "\u{2717}",
                                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                            ),
                            JobStatus::Running => (
                                "\u{25b6}",
                                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                            ),
                            JobStatus::Pending => ("\u{00b7}", Style::default().fg(Color::DarkGray)),
                        };
                        let mut line = format!("{mark} {lbl}");
                        if let Some(e) = &errs[i] {
                            line.push_str(&format!("  — {e}"));
                        }
                        ListItem::new(line).style(style)
                    })
                    .collect();
                // stacked top/bottom: jobs list above, live summary panel below
                let body = Layout::vertical([Constraint::Min(4), Constraint::Length(11)])
                    .split(v[2]);
                f.render_widget(
                    List::new(items)
                        .block(Block::default().borders(Borders::ALL).title(" Jobs ")),
                    body[0],
                );
                let now = statuses
                    .iter()
                    .position(|s| matches!(s, JobStatus::Running))
                    .map(|i| labels[i].clone())
                    .unwrap_or_else(|| "\u{2014}".into());
                let g = |s: &'static str| Span::styled(s, Style::default().fg(Color::Gray));
                let summary = vec![
                    Line::from(vec![g("Progress  "), Span::styled(format!("{completed}/{n}"), Style::default().fg(Color::White).add_modifier(Modifier::BOLD))]),
                    Line::from(vec![g("Elapsed   "), Span::raw(build::fmt_dur(elapsed))]),
                    Line::from(vec![g("ETA       "), Span::styled(eta_s.clone(), Style::default().fg(Color::Cyan))]),
                    Line::from(vec![g("Failed    "), Span::styled(format!("{failures}"), Style::default().fg(if failures > 0 { Color::Red } else { Color::Green }))]),
                    Line::raw(""),
                    Line::from(g("Now building")),
                    Line::from(Span::styled(now, Style::default().fg(Color::Yellow))),
                ];
                f.render_widget(
                    Paragraph::new(summary)
                        .wrap(Wrap { trim: true })
                        .block(Block::default().borders(Borders::ALL).title(" Summary ")),
                    body[1],
                );

                let footer = if finished {
                    if failures > 0 {
                        format!(
                            " {} job(s) in {} — {} FAILED · press any key ",
                            n,
                            build::fmt_dur(elapsed),
                            failures
                        )
                    } else {
                        format!(" {} job(s) in {} · press any key ", n, build::fmt_dur(elapsed))
                    }
                } else {
                    " building… (q to stop after current job) ".to_string()
                };
                let fstyle = if finished && failures > 0 {
                    Style::default().fg(Color::White).bg(Color::Red)
                } else if finished {
                    Style::default().fg(Color::Black).bg(Color::Green)
                } else {
                    Style::default().fg(Color::Gray)
                };
                f.render_widget(
                    Paragraph::new(Line::from(Span::styled(footer, fstyle)))
                        .block(Block::default().borders(Borders::ALL).title(" Status ")),
                    v[3],
                );
            })?;

            if finished {
                // wait for a keypress so the result is visible
                if event::poll(Duration::from_millis(150))? {
                    if let Event::Key(_) = event::read()? {
                        break;
                    }
                }
                // also break once the worker thread is fully joined
                if handle.is_finished() {
                    // small grace so user can read; consume one key if pressed
                    if event::poll(Duration::from_millis(2000))? {
                        let _ = event::read();
                    }
                    break;
                }
            } else {
                let _ = event::poll(Duration::from_millis(100))?;
            }
        }

        handle.join().unwrap_or_else(|_| Ok(()))
    });

    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    term.show_cursor()?;
    build_result
}

/// Build the jobs for a TUI BuildReq (format-mode or edition-mode).
pub fn jobs_for_req(repo: &Repo, req: &BuildReq) -> Result<Vec<Job>> {
    let lang = (req.lang != "all").then_some(req.lang.clone());
    if req.format != "edition" {
        build::plan_format(repo, &Some(req.book.clone()), &Some(req.format.clone()), &lang)
    } else {
        let edition = (req.edition != "all").then_some(req.edition.clone());
        build::plan_editions(repo, &Some(req.book.clone()), &lang, &edition)
    }
}
