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

/// Actions the TUI can run (display name, one-line description).
const ACTIONS: &[(&str, &str)] = &[
    ("Build", "quick build of the selected format/edition — no checks"),
    ("Release", "complete: all editions + covers + deep validate"),
    ("Validate", "deep checks: epubcheck + PDF geometry + cover"),
    ("Covers", "render front + wrap covers (resvg)"),
    ("Audiobook", "render audiobook (kab engine)"),
];

/// Format selector options; "edition" means use the edition selector instead.
const FORMATS: &[&str] = &["edition", "all", "epub", "pdf", "kdp", "print"];

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
    let mut fmt_i = 0usize; // 0 = "edition" (use edition selector)
    let mut act_i = 0usize; // index into ACTIONS
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
            let is_build = matches!(action.0, "Build" | "Release");

            // command preview (action-aware)
            let langflag = if lang != "all" { format!(" --lang {lang}") } else { String::new() };
            let cmd = match action.0 {
                "Validate" => format!("bookmill validate {} --deep", book.slug),
                "Covers" => format!("bookmill build cover {}{langflag}", book.slug),
                "Audiobook" => format!("bookmill audiobook {}{langflag}", book.slug),
                "Release" => format!("bookmill build {0}{langflag} ; build cover ; validate --deep", book.slug),
                _ => {
                    let mut c = format!("bookmill build {}", book.slug);
                    if by_edition {
                        if edition != "all" {
                            c.push_str(&format!(" --edition {edition}"));
                        }
                    } else {
                        c.push_str(&format!(" --format {format}"));
                    }
                    c.push_str(&langflag);
                    c
                }
            };
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

                let cols = Layout::horizontal([Constraint::Percentage(48), Constraint::Percentage(52)]).split(v[1]);

                // books list
                let items: Vec<ListItem> = books
                    .iter()
                    .map(|b| ListItem::new(format!("{}  ·  {}", b.slug, b.title)))
                    .collect();
                f.render_stateful_widget(
                    List::new(items)
                        .block(Block::default().borders(Borders::ALL).title(" Books  (↑/↓) "))
                        .highlight_symbol("▶ ")
                        .highlight_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD | Modifier::REVERSED)),
                    cols[0],
                    &mut bsel.clone(),
                );

                // build-target panel
                let chip = |s: &str| Span::styled(format!(" {s} "), Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD));
                let key = |s: &'static str| Span::styled(s, Style::default().fg(Color::DarkGray));
                let lbl = |s: &'static str| Span::styled(s, Style::default().fg(Color::Gray));
                let ed_hint = if by_edition { "e / E change" } else { "(format mode — n/a)" };
                let act_chip = Span::styled(format!(" {} ", action.0), Style::default().fg(Color::Black).bg(Color::Magenta).add_modifier(Modifier::BOLD));
                let dim = |s: &str| Span::styled(format!(" {s} "), Style::default().fg(Color::DarkGray));
                let lines = vec![
                    Line::from(vec![lbl("Action    "), act_chip, Span::raw("  "), key("a / A change")]),
                    Line::from(vec![lbl("          "), Span::styled(action.1, Style::default().fg(Color::DarkGray))]),
                    Line::raw(""),
                    Line::from(vec![lbl("Book      "), Span::styled(&book.slug, Style::default().fg(Color::White).add_modifier(Modifier::BOLD))]),
                    Line::from(vec![lbl("Language  "), chip(&lang), Span::raw("  "), key("l / L change")]),
                    Line::from(vec![lbl("Format    "), if is_build { chip(&format) } else { dim(&format) }, Span::raw("  "), key(if is_build { "f / F change" } else { "(n/a)" })]),
                    Line::from(vec![lbl("Edition   "), if is_build && by_edition { chip(&edition) } else { dim(&edition) }, Span::raw("  "), key(if is_build { ed_hint } else { "(n/a)" })]),
                    Line::raw(""),
                    Line::from(vec![lbl(if is_build { "Builds    " } else { "Does      " }), Span::styled(if is_build { outputs } else { action.1.to_string() }, Style::default().fg(Color::Green))]),
                    Line::from(vec![lbl("Language  "), Span::styled(langs_desc, Style::default().fg(Color::Green))]),
                    Line::raw(""),
                    Line::from(vec![lbl("Runs      "), Span::styled(cmd, Style::default().fg(Color::Yellow))]),
                    Line::raw(""),
                    Line::from(Span::styled(format!("  press ⏎ Enter to run: {}  ", action.0), Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD))),
                ];
                f.render_widget(
                    Paragraph::new(lines)
                        .wrap(Wrap { trim: true })
                        .block(Block::default().borders(Borders::ALL).title(" Build target ")),
                    cols[1],
                );

                // help box
                f.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled(" ↑/↓ ", Style::default().fg(Color::Black).bg(Color::Gray)),
                        Span::raw(" book   "),
                        Span::styled(" f/F ", Style::default().fg(Color::Black).bg(Color::Gray)),
                        Span::raw(" format   "),
                        Span::styled(" l/L ", Style::default().fg(Color::Black).bg(Color::Gray)),
                        Span::raw(" language   "),
                        Span::styled(" e/E ", Style::default().fg(Color::Black).bg(Color::Gray)),
                        Span::raw(" edition   "),
                        Span::styled(" a/A ", Style::default().fg(Color::Black).bg(Color::Magenta)),
                        Span::raw(" action   "),
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
                match k.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Down | KeyCode::Char('j') => {
                        bsel.select(Some((bi + 1).min(nb - 1)));
                        lang_i = 0;
                        ed_i = 0;
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        bsel.select(Some(bi.saturating_sub(1)));
                        lang_i = 0;
                        ed_i = 0;
                    }
                    KeyCode::Char('f') | KeyCode::Right => fmt_i = (fmt_i + 1) % FORMATS.len(),
                    KeyCode::Char('F') | KeyCode::Left => fmt_i = (fmt_i + FORMATS.len() - 1) % FORMATS.len(),
                    KeyCode::Char('l') => lang_i = (lang_i + 1) % lopts.len(),
                    KeyCode::Char('L') => lang_i = (lang_i + lopts.len() - 1) % lopts.len(),
                    KeyCode::Char('e') => ed_i = (ed_i + 1) % eopts.len(),
                    KeyCode::Char('E') => ed_i = (ed_i + eopts.len() - 1) % eopts.len(),
                    KeyCode::Char('a') => act_i = (act_i + 1) % ACTIONS.len(),
                    KeyCode::Char('A') => act_i = (act_i + ACTIONS.len() - 1) % ACTIONS.len(),
                    KeyCode::Enter => {
                        let b = book.slug.clone();
                        result = Some(match action.0 {
                            "Release" => Action::Release(BuildReq { book: b, lang, edition, format }),
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
pub fn run_queue_ui(repo: &Repo, jobs: &[Job]) -> Result<()> {
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
                        Span::styled("  building queue", Style::default().fg(Color::Gray)),
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
                // horizontal split: jobs list | live summary panel
                let body = Layout::horizontal([Constraint::Percentage(64), Constraint::Percentage(36)])
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
