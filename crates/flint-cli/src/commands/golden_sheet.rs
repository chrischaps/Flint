//! The goldens' contact sheet (#329): one `index.html` in a suite's out
//! dir with each case's golden, render and diff side by side, its numbers
//! against its tolerance, and the failures first. Reviewing a `--bless`
//! is reading one page instead of opening a hundred files.
//!
//! The page is self-contained but for the images, which it reads beside
//! itself: `<case>.png` (the render), `<case>.diff.png` (written when any
//! pixel moved) and `golden/<case>.png` (the golden as it was before the
//! run, so a bless still shows what it replaced). The whole out dir can
//! be zipped, attached or uploaded as it is.

use super::render_goldens::CaseReport;

pub struct Sheet<'a> {
    pub suite: &'a str,
    pub manifest: &'a str,
    pub bless: bool,
    pub isolate: bool,
    pub seconds: f64,
    pub no_gpu: bool,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// `s` without ANSI escape sequences (a spawned case's log is coloured).
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            // Parameters, then one final byte in @..~
            for d in chars.by_ref() {
                if ('@'..='~').contains(&d) {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Where a case sorts: the ones to look at first.
fn rank(r: &CaseReport) -> u8 {
    match r.status {
        "error" => 0,
        "fail" | "missing" => 1,
        _ if moved(r) => 2,
        _ => 3,
    }
}

fn moved(r: &CaseReport) -> bool {
    r.moved()
}

/// The filter a case belongs to.
fn group(r: &CaseReport) -> &'static str {
    match rank(r) {
        0 | 1 => "failed",
        2 => "moved",
        _ if r.status == "skip" => "skipped",
        _ => "identical",
    }
}

/// A meter: `value` against `limit`, the fill capped at twice the limit so
/// a pass reads as under half the track and a fail as over it.
fn meter(label: &str, value: f64, limit: f64, shown: &str, limit_shown: &str) -> String {
    let ratio = if limit > 0.0 {
        value / limit
    } else if value > 0.0 {
        2.0
    } else {
        0.0
    };
    let fill = (ratio / 2.0).clamp(0.0, 1.0) * 100.0;
    let class = if ratio > 1.0 {
        "over"
    } else if value > 0.0 {
        "under"
    } else {
        "zero"
    };
    format!(
        r#"<div class="meter {class}"><span class="m-label">{label}</span><span class="track"><span class="fill" style="width:{fill:.1}%"></span><span class="limit"></span></span><span class="m-value">{shown} <em>/ {limit_shown}</em></span></div>"#
    )
}

fn frame(kind: &str, src: Option<String>, empty: &str, name: &str) -> String {
    match src {
        Some(src) => format!(
            r#"<figure class="frame {kind}"><a href="{src}" target="_blank"><img src="{src}" alt="{kind} of {name}"></a><figcaption>{kind}</figcaption></figure>"#,
            src = esc(&src),
            name = esc(name),
        ),
        None => format!(
            r#"<figure class="frame {kind} none"><div class="hole">{empty}</div><figcaption>{kind}</figcaption></figure>"#
        ),
    }
}

fn case_html(r: &CaseReport, bless: bool) -> String {
    let name = esc(&r.name);
    let rendered = r.status != "skip" && r.status != "error";
    // A --bless-drifted blesses some cases: only theirs is "was"
    let golden_label = if bless && r.status == "blessed" {
        "was"
    } else {
        "golden"
    };
    let golden = frame(
        golden_label,
        r.had_golden.then(|| format!("golden/{}.png", r.name)),
        "no golden yet",
        &r.name,
    );
    let render = frame(
        "render",
        rendered.then(|| format!("{}.png", r.name)),
        if r.status == "skip" {
            "skipped"
        } else {
            "no render"
        },
        &r.name,
    );
    let diff = frame(
        "diff",
        r.diff.as_ref().map(|_| format!("{}.diff.png", r.name)),
        if r.comparison.is_some() {
            "identical"
        } else {
            "&mdash;"
        },
        &r.name,
    );

    let mut metrics = String::new();
    if let Some(c) = &r.comparison {
        let t = &r.tolerance;
        metrics.push_str(&meter(
            &format!("past {}", t.level),
            c.far_share,
            t.share,
            &format!("{:.2}%", c.far_share * 100.0),
            &format!("{:.2}%", t.share * 100.0),
        ));
        metrics.push_str(&meter(
            "mean",
            c.mean,
            t.mean,
            &format!("{:.2}", c.mean),
            &format!("{:.2}", t.mean),
        ));
        metrics.push_str(&format!(
            r#"<div class="facts"><span><b>{}</b> px moved</span><span>max <b>{}</b></span><span><b>{}</b> ms</span></div>"#,
            c.changed, c.max, r.ms
        ));
    } else if r.ms > 0 {
        metrics.push_str(&format!(
            r#"<div class="facts"><span><b>{}</b> ms</span></div>"#,
            r.ms
        ));
    }
    if let Some(t) = &r.text {
        metrics.push_str(&meter(
            &format!("text {}&times;{}", t.size[0], t.size[1]),
            t.cells as f64,
            r.text_cells as f64,
            &format!("{} cell{}", t.cells, if t.cells == 1 { "" } else { "s" }),
            &r.text_cells.to_string(),
        ));
    }

    let args = if r.args.is_empty() {
        String::new()
    } else {
        format!(" <code>{}</code>", esc(&r.args.join(" ")))
    };
    let message = r
        .message
        .as_ref()
        .filter(|_| r.status != "skip")
        .map(|m| format!(r#"<pre class="message">{}</pre>"#, esc(&strip_ansi(m))))
        .unwrap_or_default();
    let text_diff = r
        .text_diff
        .as_ref()
        .map(|d| {
            format!(
                r#"<details class="text-diff"><summary>the grid's moved cells</summary><pre>{}</pre></details>"#,
                esc(d)
            )
        })
        .unwrap_or_default();
    let blink = if r.had_golden && rendered {
        r#"<button class="blink" type="button" title="Flip the render with its golden">blink</button>"#
    } else {
        ""
    };
    let status = match r.status {
        "blessed" if !moved(r) && r.had_golden => "blessed, unchanged",
        "blessed" if r.drift => "drift, blessed",
        "pass" if r.drift => "pass, drifted",
        s => s,
    };
    format!(
        r##"<article class="case {group} st-{st}" id="{name}" data-group="{group}">
  <header><h2><a href="#{name}">{name}</a></h2><span class="chip">{status}</span>{blink}</header>
  <p class="what"><span class="scene">{scene}</span>{args}</p>
  <div class="strip">{golden}{render}{diff}</div>
  <div class="metrics">{metrics}</div>{text_diff}{message}
</article>"##,
        group = group(r),
        st = r.status,
        scene = esc(&r.scene),
    )
}

/// The page for `reports`.
pub fn html(sheet: &Sheet, reports: &[CaseReport]) -> String {
    let mut order: Vec<&CaseReport> = reports.iter().collect();
    order.sort_by_key(|r| rank(r)); // stable: manifest order within a rank
    let count = |g: &str| reports.iter().filter(|r| group(r) == g).count();
    let (failed, moved_n, identical, skipped) = (
        count("failed"),
        count("moved"),
        count("identical"),
        count("skipped"),
    );
    let verdict = if sheet.no_gpu {
        "skipped: no GPU adapter".to_string()
    } else if sheet.bless {
        let blessed = reports.iter().filter(|r| r.status == "blessed").count();
        format!("blessed {blessed} case(s)")
    } else if failed > 0 {
        format!("{failed} failed")
    } else {
        let drifted = reports.iter().filter(|r| r.drift).count();
        if drifted == 0 {
            "every case holds, byte-identical".to_string()
        } else {
            format!("every case holds; {drifted} drifted inside the tolerance")
        }
    };
    let mode = if sheet.isolate {
        "a process a case"
    } else {
        "one process"
    };
    let bless_note = if sheet.bless {
        r#"<p class="note">This run blessed: each <b>was</b> frame is the golden it replaced, and the diff is what the bless changed. Look at every moved case before committing.</p>"#
    } else {
        ""
    };
    let cases: String = order.iter().map(|r| case_html(r, sheet.bless)).collect();
    let filter = |key: &str, label: &str, n: usize| {
        format!(
            r#"<button type="button" data-filter="{key}"{}>{label} <span>{n}</span></button>"#,
            if n == 0 && key != "all" {
                " disabled"
            } else {
                ""
            }
        )
    };
    format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{suite} goldens</title>
<style>
:root {{
  --bg: #15120f; --paper: #1e1a16; --ink: #ece4d8; --dim: #9a8f80; --line: #3a322a;
  --safe: #e8a33d; --fail: #ef5a4c; --ok: #7fbf7a; --hole: #0d0b09;
  color-scheme: dark;
}}
@media (prefers-color-scheme: light) {{
  :root {{ --bg: #efe9df; --paper: #fbf8f2; --ink: #231d16; --dim: #6f6556; --line: #d8cfc1;
    --safe: #b5701a; --fail: #c0392b; --ok: #3f7f3a; --hole: #e4ddd1; color-scheme: light; }}
}}
* {{ box-sizing: border-box; }}
body {{ margin: 0; background: var(--bg); color: var(--ink);
  font: 15px/1.45 ui-sans-serif, system-ui, "Segoe UI", sans-serif; }}
main {{ max-width: 1500px; margin: 0 auto; padding: 28px 16px 64px; }}
.top h1 {{ font: 600 28px/1.1 ui-serif, Georgia, serif; margin: 0 0 6px; letter-spacing: .01em; }}
.top h1 small {{ color: var(--safe); font: 500 13px ui-monospace, monospace; letter-spacing: .12em;
  text-transform: uppercase; display: block; margin-bottom: 8px; }}
.top p {{ margin: 2px 0; color: var(--dim); }}
.top .verdict {{ color: var(--ink); font-size: 18px; }}
.note {{ border-left: 3px solid var(--safe); padding: 6px 12px; background: var(--paper); color: var(--ink); }}
nav {{ position: sticky; top: 0; z-index: 2; display: flex; gap: 6px; flex-wrap: wrap;
  padding: 12px 0; margin: 14px 0 10px; background: var(--bg); border-bottom: 1px solid var(--line); }}
nav button, .blink {{ font: 13px ui-monospace, monospace; background: var(--paper); color: var(--ink);
  border: 1px solid var(--line); border-radius: 999px; padding: 4px 12px; cursor: pointer; }}
nav button span {{ color: var(--dim); }}
nav button[aria-pressed="true"] {{ border-color: var(--safe); color: var(--safe); }}
nav button:disabled {{ opacity: .4; cursor: default; }}
.case {{ background: var(--paper); border: 1px solid var(--line); border-radius: 6px;
  padding: 14px 16px 16px; margin: 0 0 16px; }}
.case.failed {{ border-color: color-mix(in srgb, var(--fail) 55%, var(--line)); }}
.case header {{ display: flex; align-items: center; gap: 10px; flex-wrap: wrap; }}
.case h2 {{ font: 600 16px ui-monospace, monospace; margin: 0; }}
.case h2 a {{ color: inherit; text-decoration: none; }}
.chip {{ font: 12px ui-monospace, monospace; padding: 2px 9px; border-radius: 999px;
  border: 1px solid var(--line); color: var(--dim); }}
.failed .chip {{ color: var(--fail); border-color: var(--fail); }}
.moved .chip {{ color: var(--safe); border-color: var(--safe); }}
.identical .chip {{ color: var(--ok); border-color: color-mix(in srgb, var(--ok) 60%, var(--line)); }}
.blink {{ margin-left: auto; }}
.blink[aria-pressed="true"] {{ border-color: var(--safe); color: var(--safe); }}
.what {{ margin: 6px 0 12px; color: var(--dim); font-size: 13px; overflow-wrap: anywhere; }}
.what code {{ font: 12px ui-monospace, monospace; color: var(--ink); background: var(--bg);
  padding: 1px 6px; border-radius: 3px; }}
.strip {{ display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 10px;
  background: var(--hole); padding: 10px; border-radius: 4px;
  background-image: radial-gradient(circle at 6px 50%, var(--bg) 2.5px, transparent 3px);
  background-size: 100% 14px; }}
.frame {{ margin: 0; min-width: 0; }}
.frame img {{ display: block; width: 100%; height: auto; image-rendering: pixelated;
  border-radius: 2px; background: #000; }}
.frame .hole {{ aspect-ratio: 16 / 9; display: grid; place-items: center; color: var(--dim);
  border: 1px dashed var(--line); border-radius: 2px; font: 13px ui-monospace, monospace; }}
.frame figcaption {{ font: 11px ui-monospace, monospace; color: var(--dim); text-transform: uppercase;
  letter-spacing: .14em; margin-top: 5px; }}
.metrics {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
  gap: 6px 20px; margin-top: 12px; align-items: center; }}
.meter {{ display: grid; grid-template-columns: 82px 1fr auto; gap: 10px; align-items: center;
  font: 12px ui-monospace, monospace; }}
.m-label {{ color: var(--dim); }}
.m-value em {{ color: var(--dim); font-style: normal; }}
.track {{ position: relative; height: 6px; background: var(--bg); border-radius: 3px; }}
.fill {{ position: absolute; inset: 0 auto 0 0; border-radius: 3px; background: var(--ok); }}
.under .fill {{ background: var(--safe); }}
.over .fill {{ background: var(--fail); }}
.limit {{ position: absolute; left: 50%; top: -3px; bottom: -3px; width: 2px; background: var(--dim); }}
.facts {{ display: flex; gap: 16px; font: 12px ui-monospace, monospace; color: var(--dim); }}
.facts b {{ color: var(--ink); font-weight: 600; }}
pre {{ font: 12px/1.35 ui-monospace, monospace; overflow-x: auto; background: var(--bg);
  padding: 10px; border-radius: 4px; margin: 10px 0 0; }}
.message {{ color: var(--fail); white-space: pre-wrap; max-height: 260px; }}
.text-diff summary {{ cursor: pointer; color: var(--safe); font: 12px ui-monospace, monospace; margin-top: 10px; }}
.hidden {{ display: none; }}
footer {{ color: var(--dim); font-size: 12px; margin-top: 30px; }}
@media (max-width: 720px) {{
  .strip {{ grid-template-columns: 1fr; }}
  .meter {{ grid-template-columns: 70px 1fr auto; }}
}}
</style>
</head>
<body>
<main>
<section class="top">
  <h1><small>flint render-suite &middot; contact sheet</small>{suite}</h1>
  <p class="verdict">{verdict}</p>
  <p>{total} case(s) in {seconds:.1} s, {mode} &middot; <code>{manifest}</code></p>
  {bless_note}
</section>
<nav>
  {f_all}{f_failed}{f_moved}{f_identical}{f_skipped}
</nav>
{cases}
<footer>Diff: the golden dimmed to grey where a pixel holds, amber where it moved, red past the case's level. A meter's tick is its limit. <b>blink</b> flips a render with its golden; click a frame for full size.</footer>
</main>
<script>
(() => {{
  const buttons = document.querySelectorAll('nav button');
  const show = key => {{
    buttons.forEach(b => b.setAttribute('aria-pressed', String(b.dataset.filter === key)));
    document.querySelectorAll('.case').forEach(c =>
      c.classList.toggle('hidden', key !== 'all' && c.dataset.group !== key));
  }};
  buttons.forEach(b => b.addEventListener('click', () => show(b.dataset.filter)));
  show({start});
  document.querySelectorAll('.blink').forEach(btn => {{
    let timer = null;
    btn.addEventListener('click', () => {{
      const card = btn.closest('.case');
      const img = card.querySelector('.frame.render img');
      const cap = card.querySelector('.frame.render figcaption');
      const golden = card.querySelector('.frame.golden img, .frame.was img');
      if (!img || !golden) return;
      if (timer) {{
        clearInterval(timer); timer = null; img.src = img.dataset.own; cap.textContent = 'render';
        btn.setAttribute('aria-pressed', 'false'); return;
      }}
      img.dataset.own = img.getAttribute('src');
      let on = false;
      timer = setInterval(() => {{
        on = !on;
        img.src = on ? golden.getAttribute('src') : img.dataset.own;
        cap.textContent = on ? 'golden' : 'render';
      }}, 450);
      btn.setAttribute('aria-pressed', 'true');
    }});
  }});
}})();
</script>
</body>
</html>
"##,
        suite = esc(sheet.suite),
        manifest = esc(sheet.manifest),
        total = reports.len(),
        seconds = sheet.seconds,
        f_all = filter("all", "all", reports.len()),
        f_failed = filter("failed", "failed", failed),
        f_moved = filter("moved", "drifted", moved_n),
        f_identical = filter("identical", "identical", identical),
        f_skipped = if skipped > 0 {
            filter("skipped", "skipped", skipped)
        } else {
            String::new()
        },
        start = if failed > 0 { "'failed'" } else { "'all'" },
    )
}

#[cfg(test)]
mod tests {
    use super::super::render_goldens::{Comparison, Tolerance};
    use super::*;

    fn case(name: &str, status: &'static str, changed: usize) -> CaseReport {
        CaseReport {
            name: name.into(),
            scene: "demo/x.scene.toml".into(),
            args: vec!["--style".into(), "ascii".into()],
            status,
            drift: status == "pass" && changed > 0,
            comparison: Some(Comparison {
                changed,
                far_share: if status == "fail" { 0.5 } else { 0.0 },
                ..Default::default()
            }),
            tolerance: Tolerance::default(),
            render: String::new(),
            golden: String::new(),
            variant: None,
            adapter: None,
            diff: (changed > 0).then(String::new),
            text: None,
            text_cells: 0,
            text_diff: None,
            message: None,
            ms: 12,
            had_golden: true,
        }
    }

    #[test]
    fn logs_lose_their_colour_codes() {
        assert_eq!(
            strip_ansi("\u{1b}[2m12:00\u{1b}[0m \u{1b}[33m WARN\u{1b}[0m ok"),
            "12:00  WARN ok"
        );
    }

    #[test]
    fn failures_lead_the_sheet_and_each_case_shows_three_frames() {
        let reports = vec![
            case("still", "pass", 0),
            case("drifted", "pass", 40),
            case("<broke>", "fail", 900),
        ];
        let page = html(
            &Sheet {
                suite: "stylize",
                manifest: "m.goldens.toml",
                bless: false,
                isolate: false,
                seconds: 1.0,
                no_gpu: false,
            },
            &reports,
        );
        let at = |s: &str| page.find(s).unwrap_or_else(|| panic!("{s} missing"));
        assert!(at("id=\"&lt;broke&gt;\"") < at("id=\"drifted\""));
        assert!(at("id=\"drifted\"") < at("id=\"still\""));
        assert!(page.contains(r#"src="golden/still.png""#));
        assert!(page.contains(r#"src="drifted.diff.png""#));
        assert!(
            !page.contains(r#"src="still.diff.png""#),
            "an identical case writes no diff"
        );
        assert!(page.contains("1 failed") && page.contains("show('failed')"));
        assert!(
            page.contains("pass, drifted"),
            "a passing case that moved says so"
        );
    }

    #[test]
    fn a_clean_run_says_byte_identical_and_a_drifted_one_counts() {
        let sheet = Sheet {
            suite: "stylize",
            manifest: "m.goldens.toml",
            bless: false,
            isolate: false,
            seconds: 1.0,
            no_gpu: false,
        };
        let clean = html(&sheet, &[case("a", "pass", 0), case("b", "pass", 0)]);
        assert!(clean.contains("every case holds, byte-identical"));
        let drifted = html(&sheet, &[case("a", "pass", 0), case("b", "pass", 7)]);
        assert!(drifted.contains("1 drifted inside the tolerance"));
    }
}
