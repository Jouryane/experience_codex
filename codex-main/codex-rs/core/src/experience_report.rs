//! Minimal read-only HTML report for the Experience store.
//!
//! This is the "look at what has been recorded" surface: one self-contained
//! file, no server, no app-server, no writes. It reads the same canonical
//! store the running agent executes from ([`crate::experience_paths`]) plus the
//! usage ledger next to it, and renders:
//!
//! - every recorded artifact (exact experiences *and* templates),
//! - its status / pin / scope / confidence,
//! - its execution records (band, outcome, template bindings, audit trail),
//! - its full body (trigger, parameters, preconditions, workflow,
//!   postconditions, verification, provenance).
//!
//! Two rules it does not break:
//!
//! 1. **No invention.** Every number shown is either read from the store or
//!    derived from the usage ledger with the formula printed next to it. The
//!    store has no evidence-based confidence field, so the report shows the
//!    management-set value as "user confidence" and the usage-derived rate as
//!    a separate, explicitly-labelled number.
//! 2. **No writes.** Opening the report cannot change anything.

use std::path::Path;

use experience_core::domain::experience::Experience as CanonicalExperience;
use experience_core::domain::experience::VerificationStep;
use experience_core::domain::template::ExperienceTemplate;
use experience_core::store::ExperienceStore;

use crate::experience_management::ExperienceUsageStore;
use crate::experience_management::UsageEntry;
use crate::experience_management::UsageLog;

/// One row of the report: either an executable instance or a template.
enum Artifact<'a> {
    Experience(&'a CanonicalExperience),
    Template(&'a ExperienceTemplate),
}

impl Artifact<'_> {
    fn name(&self) -> &str {
        match self {
            Self::Experience(experience) => &experience.name,
            Self::Template(template) => &template.name,
        }
    }

    fn status(&self) -> experience_core::domain::experience::ExperienceStatus {
        match self {
            Self::Experience(experience) => experience.status,
            Self::Template(template) => template.status,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Experience(_) => "exact",
            Self::Template(_) => "template",
        }
    }

    fn workflow(&self) -> &[experience_core::domain::experience::WorkflowStep] {
        match self {
            Self::Experience(experience) => &experience.workflow,
            Self::Template(template) => &template.workflow,
        }
    }

    fn preconditions(&self) -> &[experience_core::domain::predicate::Predicate] {
        match self {
            Self::Experience(experience) => &experience.preconditions,
            Self::Template(template) => &template.preconditions,
        }
    }

    fn postconditions(&self) -> &[experience_core::domain::predicate::Predicate] {
        match self {
            Self::Experience(experience) => &experience.postconditions,
            Self::Template(template) => &template.postconditions,
        }
    }

    fn verification(&self) -> &[VerificationStep] {
        match self {
            Self::Experience(experience) => &experience.verification,
            Self::Template(template) => &template.verification,
        }
    }

    fn trigger(&self) -> (&str, Option<&str>) {
        match self {
            Self::Experience(experience) => (
                experience.trigger.tool.as_str(),
                experience.trigger.command_pattern.as_deref(),
            ),
            Self::Template(template) => (
                template.trigger.tool.as_str(),
                template.trigger.command_pattern.as_deref(),
            ),
        }
    }

    fn parameters(&self) -> &[experience_core::domain::template::TemplateParameter] {
        match self {
            Self::Experience(_) => &[],
            Self::Template(template) => &template.parameters,
        }
    }
}

/// Render the store at `store_path` (and the `usage.json` beside it).
pub fn render_html(store_path: &Path) -> anyhow::Result<String> {
    let store = ExperienceStore::open(store_path)
        .map_err(|error| anyhow::anyhow!("cannot open store {}: {error}", store_path.display()))?;
    let usage_path = store_path
        .parent()
        .map(|parent| parent.join("usage.json"))
        .unwrap_or_else(|| std::path::PathBuf::from("usage.json"));
    let usage = ExperienceUsageStore::load_from_path(&usage_path);

    let mut artifacts: Vec<Artifact<'_>> = Vec::new();
    artifacts.extend(store.all().iter().map(Artifact::Experience));
    artifacts.extend(store.templates().iter().map(Artifact::Template));
    // Active first, then most-used, then name: what is in force belongs on top.
    artifacts.sort_by(|left, right| {
        let rank = |artifact: &Artifact<'_>| match artifact.status() {
            experience_core::domain::experience::ExperienceStatus::Active => 0,
            experience_core::domain::experience::ExperienceStatus::Decaying => 1,
            _ => 2,
        };
        let hits = |artifact: &Artifact<'_>| {
            usage
                .entries
                .get(artifact.name())
                .map(|entry| entry.hits)
                .unwrap_or(0)
        };
        rank(left)
            .cmp(&rank(right))
            .then_with(|| hits(right).cmp(&hits(left)))
            .then_with(|| left.name().cmp(right.name()))
    });

    let mut body = String::new();
    for artifact in &artifacts {
        render_artifact(&mut body, &store, &usage, artifact);
    }
    if artifacts.is_empty() {
        body.push_str(
            "<p class=\"empty\">这个 store 里还没有记录任何经验。<br>\
             先让改版 Codex 跑一次任务：每一次被经验接管的执行都会记到这里；\
             同一个任务族成功跑两次之后，可以归纳出一个候选模板。</p>",
        );
    }

    let totals = totals(&usage, &artifacts);
    let policy_text = policy_summary(&store.global_policy());
    Ok(page(
        store_path,
        &usage_path,
        &artifacts,
        &totals,
        &policy_text,
        &body,
    ))
}

#[derive(Default)]
struct Totals {
    hits: u64,
    misfires: u64,
    invalid: u64,
}

fn totals(usage: &ExperienceUsageStore, artifacts: &[Artifact<'_>]) -> Totals {
    let mut totals = Totals::default();
    for artifact in artifacts {
        if let Some(entry) = usage.entries.get(artifact.name()) {
            totals.hits += u64::from(entry.hits);
            totals.misfires += u64::from(entry.misfires);
            totals.invalid += u64::from(entry.invalid_failures);
        }
    }
    totals
}

fn render_artifact(
    out: &mut String,
    store: &ExperienceStore,
    usage: &ExperienceUsageStore,
    artifact: &Artifact<'_>,
) {
    let name = artifact.name();
    let display = store.display_name_of(name).unwrap_or(name);
    let entry = usage.entries.get(name).cloned().unwrap_or_default();
    let status = format!("{:?}", artifact.status());
    let pinned = store.is_pinned(name);
    let scope = store.scope_of(name).unwrap_or("(everywhere)");
    let confidence = store
        .user_confidence_of(name)
        .map(|value| format!("{value:.2}"))
        .unwrap_or_else(|| "—".to_string());
    let (tool, pattern) = artifact.trigger();

    out.push_str(&format!(
        "<section class=\"card\" id=\"{anchor}\" data-name=\"{search}\">\n",
        anchor = escape(&anchor_of(name)),
        search = escape(&format!("{name} {display} {status} {tool}").to_lowercase()),
    ));
    out.push_str(&format!(
        "<h2>{display}<span class=\"badge kind\">{kind}</span>\
         <span class=\"badge status {status_class}\">{status}</span>{pinned_badge}</h2>\n",
        display = escape(display),
        kind = artifact.kind(),
        status_class = escape(&status.to_lowercase()),
        pinned_badge = if pinned {
            "<span class=\"badge pin\">pinned</span>"
        } else {
            ""
        },
    ));
    out.push_str(&format!(
        "<p class=\"ident\"><code>{name}</code> · scope: {scope}</p>\n",
        name = escape(name),
        scope = escape(scope),
    ));

    // Facts grid: confidence, evidence, trigger, provenance.
    out.push_str("<div class=\"facts\">");
    fact(
        out,
        "置信度（管理面设置）",
        &confidence,
        Some("管理面直接赋值，不作为执行证据"),
    );
    fact(
        out,
        "命中次数",
        &entry.hits.to_string(),
        Some("被记录下来的接管次数"),
    );
    fact(
        out,
        "未验证 / 无法执行",
        &format!("{} / {}", entry.misfires, entry.invalid_failures),
        Some("执行了但后置没验证过 / 连执行都没能开始"),
    );
    let derived = derived_success_rate(&entry);
    fact(out, "已验证比例", &derived.0, Some(&derived.1));
    fact(
        out,
        "最近使用",
        &entry
            .last_used
            .map(|seconds| time_tag(seconds))
            .unwrap_or_else(|| "—".to_string()),
        None,
    );
    fact(
        out,
        "触发条件",
        &match pattern {
            Some(pattern) => format!("{tool} contains \"{pattern}\""),
            None => format!("{tool} (any)"),
        },
        None,
    );
    if let Some(origin) = store.candidate_origin_of(name) {
        fact(
            out,
            "来源",
            &format!("{} @ {}", origin.distiller, time_tag(origin.recorded_at)),
            Some(&truncate(&origin.task_signature, 120)),
        );
    }
    out.push_str("</div>\n");

    // Execution records.
    render_logs(out, &entry);
    render_audit(out, usage, name);

    // Body.
    out.push_str("<details class=\"body\"><summary>详情</summary>\n");
    if !artifact.parameters().is_empty() {
        out.push_str("<h4>参数</h4><table class=\"kv\"><tr><th>名称</th><th>来源</th><th>类型</th><th>捕获规则</th></tr>");
        for parameter in artifact.parameters() {
            let capture = match (&parameter.prefix, &parameter.suffix) {
                (Some(prefix), Some(suffix)) => format!("取「{prefix}」之后、「{suffix}」之前"),
                (Some(prefix), None) => format!("取「{prefix}」之后"),
                (None, Some(suffix)) => format!("取「{suffix}」之前"),
                (None, None) => "（整段文本）".to_string(),
            };
            out.push_str(&format!(
                "<tr><td><code>{}</code></td><td>{}</td><td>{}</td><td>{}</td></tr>",
                escape(&parameter.name),
                escape(&parameter.source),
                escape(&parameter.kind),
                escape(&capture),
            ));
        }
        out.push_str("</table>");
    }
    predicates(out, "前置条件", artifact.preconditions());
    out.push_str("<h4>工作流</h4><ol class=\"steps\">");
    for step in artifact.workflow() {
        out.push_str(&format!(
            "<li><code>{}</code> <span class=\"args\">{}</span></li>",
            escape(&step.action),
            escape(&step.args.to_string()),
        ));
    }
    out.push_str("</ol>");
    predicates(out, "后置条件", artifact.postconditions());
    if !artifact.verification().is_empty() {
        out.push_str("<h4>验证</h4><ul class=\"preds\">");
        for step in artifact.verification() {
            let text = match step {
                VerificationStep::ReadFile {
                    path,
                    expect_content,
                } => match expect_content {
                    Some(content) => format!("读取 {path}，期望内容 {content}"),
                    None => format!("读取 {path}"),
                },
                VerificationStep::Probe { predicate } => {
                    format!("探测 {} = {}", predicate.key, predicate.expected)
                }
            };
            out.push_str(&format!("<li>{}</li>", escape(&text)));
        }
        out.push_str("</ul>");
    }
    match artifact {
        Artifact::Experience(experience) => {
            out.push_str("<h4>原始 JSON</h4><pre>");
            out.push_str(&escape(
                &serde_json::to_string_pretty(experience).unwrap_or_default(),
            ));
            out.push_str("</pre>");
        }
        Artifact::Template(template) => {
            out.push_str("<h4>原始 JSON</h4><pre>");
            out.push_str(&escape(
                &serde_json::to_string_pretty(template).unwrap_or_default(),
            ));
            out.push_str("</pre>");
        }
    }
    out.push_str("</details>\n</section>\n");
}

/// Success rate derived from the usage ledger. The formula is printed with the
/// number so nobody has to guess where it came from.
fn derived_success_rate(entry: &UsageEntry) -> (String, String) {
    if entry.hits == 0 {
        return (
            "还没有使用记录".to_string(),
            "usage.json 里没有这条经验的条目".to_string(),
        );
    }
    let failures = entry.misfires.saturating_add(entry.invalid_failures);
    let verified = entry.hits.saturating_sub(failures);
    let rate = f64::from(verified) / f64::from(entry.hits) * 100.0;
    (
        format!("{verified}/{} ({rate:.0}%)", entry.hits),
        "(hits − misfires − invalid) / hits, from recorded usage only".to_string(),
    )
}

fn render_logs(out: &mut String, entry: &UsageEntry) {
    out.push_str("<h4>执行记录</h4>");
    if entry.logs.is_empty() {
        out.push_str("<p class=\"muted\">还没有任何接管记录。</p>");
        return;
    }
    out.push_str(
        "<table class=\"logs\"><tr><th>时间</th><th>档位</th><th>结果</th>\
         <th>进程</th><th>绑定参数 / 指纹</th></tr>",
    );
    for log in entry.logs.iter().rev() {
        render_log_row(out, log);
    }
    out.push_str("</table>");
    let bands = entry
        .decisions
        .iter()
        .map(|(band, count)| format!("{band}: {count}"))
        .collect::<Vec<_>>()
        .join(" · ");
    if !bands.is_empty() {
        out.push_str(&format!("<p class=\"muted\">档位统计 — {}</p>", escape(&bands)));
    }
}

fn render_log_row(out: &mut String, log: &UsageLog) {
    let outcome = log.outcome.clone().unwrap_or_else(|| "—".to_string());
    let result_class = match outcome.as_str() {
        "misfire" | "invalid" => "bad",
        _ => "good",
    };
    let bindings = match &log.template_audit {
        Some(audit) => {
            let pairs = audit
                .bindings
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "{}<br><span class=\"muted\">{}</span>",
                escape(&pairs),
                escape(&audit.fingerprint)
            )
        }
        None => "—".to_string(),
    };
    out.push_str(&format!(
        "<tr><td>{}</td><td>{}</td><td class=\"{result_class}\">{}</td><td>{}</td><td>{}</td></tr>",
        time_tag(log.at),
        escape(&log.band),
        escape(&outcome),
        escape(&log.process),
        bindings,
    ));
}

fn render_audit(out: &mut String, usage: &ExperienceUsageStore, name: &str) {
    let entries: Vec<_> = usage.audit.iter().filter(|entry| entry.id == name).collect();
    if entries.is_empty() {
        return;
    }
    out.push_str("<h4>管理审计</h4><table class=\"logs\"><tr><th>时间</th><th>操作</th><th>操作者</th><th>备注</th></tr>");
    for entry in entries.iter().rev() {
        out.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            time_tag(entry.at),
            escape(&entry.action),
            escape(&entry.by),
            escape(&entry.note),
        ));
    }
    out.push_str("</table>");
}

fn predicates(
    out: &mut String,
    title: &str,
    list: &[experience_core::domain::predicate::Predicate],
) {
    out.push_str(&format!("<h4>{title}</h4>"));
    if list.is_empty() {
        out.push_str("<p class=\"muted\">无</p>");
        return;
    }
    out.push_str("<ul class=\"preds\">");
    for predicate in list {
        out.push_str(&format!(
            "<li><code>{}</code> = {}</li>",
            escape(&predicate.key),
            escape(&predicate.expected.to_string()),
        ));
    }
    out.push_str("</ul>");
}

fn fact(out: &mut String, label: &str, value: &str, hint: Option<&str>) {
    out.push_str(&format!(
        "<div class=\"fact\"><span class=\"label\">{}</span>\
         <span class=\"value\">{}</span>{}</div>",
        escape(label),
        value,
        hint.map(|hint| format!("<span class=\"hint\">{}</span>", escape(hint)))
            .unwrap_or_default(),
    ));
}

/// Timestamps are rendered in the reader's own timezone by the page script;
/// the epoch stays in the markup so the file is self-describing.
fn time_tag(seconds: u64) -> String {
    format!(
        "<time data-epoch=\"{seconds}\" title=\"{seconds}\">{seconds}</time>",
    )
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let shortened: String = text.chars().take(limit).collect();
    format!("{shortened}…")
}

fn anchor_of(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect()
}

/// HTML-escape every piece of store/ledger text: experience bodies quote task
/// text and file names, which are attacker-influenced in the general case.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(character),
        }
    }
    out
}

fn policy_summary(policy: &experience_core::policy::CapabilityPolicy) -> String {
    let list = |allowed: &[String]| {
        if allowed.is_empty() {
            "(none)".to_string()
        } else {
            format!("[{}]", allowed.join(", "))
        }
    };
    format!(
        "exec={} {} · fs_write={} · fs_delete={} · network={} {} · read≤{} KiB",
        policy.exec.mode,
        list(&policy.exec.allow),
        policy.fs_write,
        policy.fs_delete,
        policy.network.mode,
        list(&policy.network.allow),
        policy.fs_read.effective_max_bytes() / 1024,
    )
}

fn page(
    store_path: &Path,
    usage_path: &Path,
    artifacts: &[Artifact<'_>],
    totals: &Totals,
    policy_text: &str,
    body: &str,
) -> String {
    let exact = artifacts
        .iter()
        .filter(|artifact| matches!(artifact, Artifact::Experience(_)))
        .count();
    let templates = artifacts.len() - exact;
    let generated = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    format!(
        r#"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Experience 记录</title>
<style>
:root {{ color-scheme: light dark; --fg:#1b1f24; --muted:#6b7280; --line:#e5e7eb; --bg:#ffffff; --card:#fbfbfd; --accent:#2563eb; }}
@media (prefers-color-scheme: dark) {{
  :root {{ --fg:#e6e8eb; --muted:#9aa3af; --line:#2b3038; --bg:#14171c; --card:#1a1e24; --accent:#7aa2f7; }}
}}
* {{ box-sizing: border-box; }}
body {{ margin:0; padding:24px; background:var(--bg); color:var(--fg);
  font: 15px/1.55 "Segoe UI", system-ui, -apple-system, "Microsoft YaHei", sans-serif; }}
main {{ max-width: 1100px; margin: 0 auto; }}
h1 {{ font-size: 22px; margin: 0 0 4px; }}
h2 {{ font-size: 17px; margin: 0 0 4px; display:flex; align-items:center; gap:8px; flex-wrap:wrap; }}
h4 {{ font-size: 13px; margin: 16px 0 6px; color:var(--muted); text-transform:none; letter-spacing:.02em; }}
code, pre, .mono {{ font-family: "Cascadia Mono", ui-monospace, Consolas, monospace; }}
code {{ font-size: 13px; }}
pre {{ background:var(--card); border:1px solid var(--line); border-radius:8px; padding:10px; overflow:auto; max-height:340px; }}
.meta {{ color:var(--muted); font-size: 13px; margin-bottom: 16px; }}
.meta div {{ margin: 2px 0; }}
.summary {{ display:flex; gap:18px; flex-wrap:wrap; padding:12px 14px; border:1px solid var(--line);
  border-radius:10px; background:var(--card); margin-bottom:16px; font-size:14px; }}
.summary b {{ font-size:16px; }}
.controls {{ display:flex; gap:8px; margin-bottom:16px; }}
input[type=search] {{ flex:1; padding:8px 10px; border:1px solid var(--line); border-radius:8px;
  background:var(--bg); color:var(--fg); font-size:14px; }}
button {{ padding:8px 12px; border:1px solid var(--line); border-radius:8px; background:var(--card);
  color:var(--fg); cursor:pointer; font-size:14px; }}
.card {{ border:1px solid var(--line); border-radius:12px; padding:16px; margin-bottom:14px; background:var(--card); }}
.ident {{ margin: 0 0 10px; color:var(--muted); font-size:13px; }}
.badge {{ font-size:11px; padding:2px 8px; border-radius:999px; border:1px solid var(--line); background:var(--bg); font-weight:600; }}
.badge.kind {{ color:var(--accent); }}
.badge.active {{ border-color:#16a34a; color:#16a34a; }}
.badge.candidate, .badge.validated, .badge.draft {{ border-color:#d97706; color:#d97706; }}
.badge.disabled, .badge.decaying {{ border-color:#dc2626; color:#dc2626; }}
.facts {{ display:grid; grid-template-columns: repeat(auto-fit, minmax(190px, 1fr)); gap:10px; margin:12px 0 4px; }}
.fact {{ border:1px solid var(--line); border-radius:8px; padding:8px 10px; background:var(--bg); }}
.fact .label {{ display:block; color:var(--muted); font-size:12px; }}
.fact .value {{ display:block; font-size:15px; }}
.fact .hint {{ display:block; color:var(--muted); font-size:11px; margin-top:2px; }}
table {{ border-collapse: collapse; width:100%; font-size:13px; margin-top:4px; }}
th, td {{ border-bottom:1px solid var(--line); padding:6px 8px; text-align:left; vertical-align:top; }}
th {{ color:var(--muted); font-weight:600; font-size:12px; }}
.logs td.good {{ color:#16a34a; }}
.logs td.bad {{ color:#dc2626; }}
.muted {{ color:var(--muted); font-size:12px; }}
.preds, .steps {{ margin:4px 0 0; padding-left:22px; font-size:13px; }}
.steps .args {{ color:var(--muted); }}
details.body {{ margin-top:12px; }}
details.body > summary {{ cursor:pointer; color:var(--accent); font-size:13px; }}
.empty {{ color:var(--muted); }}
</style>
</head>
<body>
<main>
<h1>Experience 记录</h1>
<div class="meta">
  <div>store：<code>{store}</code>（<code>{format}</code>）</div>
  <div>usage：<code>{usage}</code></div>
  <div>生成时间：<span id="generated"></span></div>
</div>
<div class="summary">
  <div><b>{total}</b> 条记录</div>
  <div><b>{exact}</b> exact / <b>{templates}</b> template</div>
  <div><b>{hits}</b> 次命中</div>
  <div><b>{misfires}</b> 次未验证</div>
  <div><b>{invalid}</b> 次无法执行</div>
</div>
<div class="meta">
  <div>能力策略（决定一条经验此刻能不能真的执行）：<code>{policy}</code></div>
</div>
<div class="controls">
  <input type="search" id="filter" placeholder="过滤：名称 / 状态 / 触发工具">
  <button id="expand">展开全部详情</button>
  <button id="collapse">收起全部详情</button>
</div>
{body}
</main>
<script>
document.querySelectorAll('time[data-epoch]').forEach(function (node) {{
  var seconds = Number(node.getAttribute('data-epoch'));
  if (!seconds) return;
  node.textContent = new Date(seconds * 1000).toLocaleString();
}});
document.getElementById('generated').textContent = new Date({generated} * 1000).toLocaleString();
var filter = document.getElementById('filter');
filter.addEventListener('input', function () {{
  var needle = filter.value.trim().toLowerCase();
  document.querySelectorAll('section.card').forEach(function (card) {{
    var haystack = card.getAttribute('data-name') || '';
    card.style.display = (!needle || haystack.indexOf(needle) !== -1) ? '' : 'none';
  }});
}});
document.getElementById('expand').addEventListener('click', function () {{
  document.querySelectorAll('details.body').forEach(function (node) {{ node.open = true; }});
}});
document.getElementById('collapse').addEventListener('click', function () {{
  document.querySelectorAll('details.body').forEach(function (node) {{ node.open = false; }});
}});
</script>
</body>
</html>
"#,
        store = escape(&store_path.display().to_string()),
        usage = escape(&usage_path.display().to_string()),
        format = escape(crate::experience_paths::detect_format(store_path).label()),
        generated = generated,
        total = artifacts.len(),
        exact = exact,
        templates = templates,
        hits = totals.hits,
        misfires = totals.misfires,
        invalid = totals.invalid,
        policy = escape(policy_text),
        body = body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use experience_core::domain::experience::ExperienceStatus;
    use experience_core::domain::experience::FailurePolicy;
    use experience_core::domain::experience::UndoPolicy;
    use experience_core::domain::template::TemplateParameter;
    use experience_core::domain::experience::WorkflowStep;
    use experience_core::domain::predicate::Predicate;
    use std::path::PathBuf;

    fn tmp_store(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("exp-report-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_template(name: &str) -> ExperienceTemplate {
        ExperienceTemplate {
            name: name.to_string(),
            trigger: experience_core::domain::action::ActionPattern {
                tool: "task".into(),
                command_pattern: Some("move it".into()),
            },
            parameters: vec![TemplateParameter {
                name: "source".into(),
                source: "task".into(),
                kind: experience_core::domain::template::PARAM_KIND_PATH.into(),
                prefix: Some("[source=".into()),
                suffix: Some("]".into()),
                required: true,
            }],
            workflow: vec![WorkflowStep::new(
                "move_file",
                serde_json::json!({ "source": "${source}", "target": "archive/x" }),
            )],
            preconditions: vec![Predicate::new("file:${source}.exists", serde_json::json!(true))],
            postconditions: vec![Predicate::new("file:archive/x.exists", serde_json::json!(true))],
            verification: vec![VerificationStep::Probe {
                predicate: Predicate::new("file:archive/x.exists", serde_json::json!(true)),
            }],
            failure_policy: FailurePolicy::StopAndReport,
            undo: UndoPolicy::Unsupported,
            status: ExperienceStatus::Candidate,
        }
    }

    #[test]
    fn report_lists_artifacts_with_usage_and_escapes_text() {
        let dir = tmp_store("render");
        let store_path = dir.join("store.json");
        let mut store = ExperienceStore::default();
        store.insert_template(sample_template("move_any")).unwrap();
        // A name that would break naive HTML interpolation.
        store
            .insert_template(sample_template("move_<script>alert(1)</script>"))
            .unwrap();
        store.save_to_path(&store_path).unwrap();

        let mut usage = ExperienceUsageStore::load_from_path(&dir.join("usage.json"));
        usage.record_logged_with_audit(
            "move_any",
            "experience_only",
            None,
            None,
            None,
            "codex-m6-task-gate".to_string(),
            None,
            Some(experience_core::domain::gate::TemplateBindingAudit {
                template: "move_any".to_string(),
                bindings: [("source".to_string(), "inbox/a.zip".to_string())]
                    .into_iter()
                    .collect(),
                fingerprint: "deadbeefdeadbeef".to_string(),
            }),
            1_789_000_000,
        );
        usage.save_to_path(&dir.join("usage.json")).unwrap();

        let html = render_html(&store_path).unwrap();
        // Structure
        assert!(html.contains("<!doctype html>"));
        assert!(html.contains("Experience 记录"));
        assert!(html.contains("<b>2</b> 条记录"));
        assert!(html.contains("执行记录"));
        assert!(html.contains("详情"));
        // Usage evidence
        assert!(html.contains("experience_only"));
        assert!(html.contains("inbox/a.zip"));
        assert!(html.contains("deadbeefdeadbeef"));
        // Policy is surfaced so "why can/can't this run" is answerable here
        assert!(html.contains("能力策略"));
        // Escaping: the injected script must not appear as markup
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_store_renders_a_hint_instead_of_failing() {
        let dir = tmp_store("empty");
        let store_path = dir.join("store.json");
        let html = render_html(&store_path).unwrap();
        assert!(html.contains("<b>0</b> 条记录"));
        assert!(html.contains("还没有记录"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
