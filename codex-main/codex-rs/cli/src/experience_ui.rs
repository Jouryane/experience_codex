//! `codex experience ui` — local web management page (P2).
//!
//! Architecture: this process spawns the SAME `codex.exe app-server --stdio`
//! as a child, speaks official JSON-RPC 2.0 to it (initialize handshake +
//! `experience/*` methods), and serves one self-contained HTML page over
//! tiny_http. The browser is only a viewer; all management goes through the
//! app-server protocol, never a private copy of the store.

use std::io::BufRead;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Write;
use std::path::Path;
use std::process::Child;
use std::process::ChildStdin;
use std::process::ChildStdout;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;

use anyhow::Context;
use anyhow::Result;
use serde_json::Value;
use serde_json::json;
use tiny_http::Header;
use tiny_http::Method;
use tiny_http::Response;
use tiny_http::Server;

pub(crate) fn run_ui(port: u16) -> Result<()> {
    let exe = std::env::current_exe().context("locate codex executable")?;
    let mut rpc = StdioRpc::spawn(&exe)?;
    rpc.handshake()?;
    let rpc = Arc::new(Mutex::new(rpc));

    let server = Server::http(format!("127.0.0.1:{port}"))
        .map_err(|error| anyhow::anyhow!("start http server: {error}"))?;
    println!("experience ui: http://127.0.0.1:{port}");
    let _ = open_browser(format!("http://127.0.0.1:{port}"));

    for request in server.incoming_requests() {
        let rpc = Arc::clone(&rpc);
        thread::spawn(move || {
            let _ = handle_request(request, &rpc);
        });
    }
    Ok(())
}

fn handle_request(
    mut request: tiny_http::Request,
    rpc: &Arc<Mutex<StdioRpc>>,
) -> Result<()> {
    let url = request.url().to_string();
    let method = request.method().clone();
    let (path, query) = match url.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (url.clone(), String::new()),
    };
    if path == "/" {
        return respond(
            request,
            200,
            "text/html; charset=utf-8",
            PAGE.as_bytes(),
        );
    }
    if path == "/api/list" {
        let value = call(rpc, "experience/list", json!({}))?;
        return respond_json(request, 200, &value);
    }
    if path == "/api/detail" {
        let id = query_param(&query, "id").unwrap_or_default();
        let value = call(rpc, "experience/detail", json!({ "id": id }))?;
        return respond_json(request, 200, &value);
    }
    if path == "/api/action" && method == Method::Post {
        let mut body = String::new();
        request
            .as_reader()
            .read_to_string(&mut body)
            .unwrap_or_default();
        let payload: Value = serde_json::from_str(&body).unwrap_or_default();
        let action = payload.get("action").and_then(Value::as_str).unwrap_or("");
        let id = payload.get("id").and_then(Value::as_str).unwrap_or_default();
        let params = match action {
            "pin" | "unpin" | "activate" | "revalidate" | "disable" | "delete" | "export" => {
                json!({ "id": id })
            }
            _ => return respond_json(request, 400, &json!({ "error": "unknown action" })),
        };
        let wire = format!("experience/{action}");
        let value = call(rpc, &wire, params)?;
        return respond_json(request, 200, &value);
    }
    if path == "/api/meta" && method == Method::Post {
        let mut body = String::new();
        request
            .as_reader()
            .read_to_string(&mut body)
            .unwrap_or_default();
        let payload: Value = serde_json::from_str(&body).unwrap_or_default();
        let params = json!({
            "id": payload.get("id").and_then(Value::as_str).unwrap_or_default(),
            "title": payload.get("title").and_then(Value::as_str),
            "note": payload.get("note").and_then(Value::as_str),
            "confidence": payload.get("confidence").and_then(Value::as_f64),
        });
        let value = call(rpc, "experience/updateMeta", params)?;
        return respond_json(request, 200, &value);
    }
    if path == "/api/control" && method == Method::Post {
        let mut body = String::new();
        request
            .as_reader()
            .read_to_string(&mut body)
            .unwrap_or_default();
        let payload: Value = serde_json::from_str(&body).unwrap_or_default();
        let params = json!({
            "id": payload.get("id").and_then(Value::as_str).unwrap_or_default(),
            "applicability": payload.get("applicability").unwrap_or(&Value::Null),
            "conditions": payload.get("conditions").unwrap_or(&Value::Null),
        });
        let value = call(rpc, "experience/updateControl", params)?;
        return respond_json(request, 200, &value);
    }
    if path == "/api/import" && method == Method::Post {
        let mut body = String::new();
        request
            .as_reader()
            .read_to_string(&mut body)
            .unwrap_or_default();
        let payload: Value = serde_json::from_str(&body).unwrap_or_default();
        let params = json!({
            "json": payload.get("json").and_then(Value::as_str).unwrap_or_default(),
            "overwrite": payload.get("overwrite").and_then(Value::as_bool).unwrap_or(false),
        });
        let value = call(rpc, "experience/import", params)?;
        return respond_json(request, 200, &value);
    }
    respond_json(request, 404, &json!({ "error": "not found" }))
}

fn call(rpc: &Arc<Mutex<StdioRpc>>, method: &str, params: Value) -> Result<Value> {
    rpc.lock()
        .unwrap_or_else(|_| panic!("rpc lock poisoned"))
        .call(method, params)
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

fn respond_json(request: tiny_http::Request, code: u16, value: &Value) -> Result<()> {
    let body = serde_json::to_vec(value)?;
    respond(request, code, "application/json; charset=utf-8", &body)
}

fn respond(
    request: tiny_http::Request,
    code: u16,
    content_type: &str,
    body: &[u8],
) -> Result<()> {
    let header = Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes())
        .map_err(|error| anyhow::anyhow!("header error: {error:?}"))?;
    let response = Response::from_data(body.to_vec())
        .with_status_code(code)
        .with_header(header);
    request.respond(response)?;
    Ok(())
}

fn open_browser(url: String) -> Result<()> {
    #[cfg(windows)]
    {
        Command::new("cmd")
            .args(["/C", "start", "", &url])
            .spawn()
            .map(|_| ())
            .context("open browser")
    }
    #[cfg(not(windows))]
    {
        let _ = url;
        Ok(())
    }
}

/// Minimal blocking JSON-RPC 2.0 client over the app-server stdio transport.
struct StdioRpc {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl StdioRpc {
    fn spawn(exe: &Path) -> Result<Self> {
        let mut child = Command::new(exe)
            .args(["app-server", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("spawn app-server")?;
        let stdin = child.stdin.take().context("app-server stdin")?;
        let stdout = child.stdout.take().context("app-server stdout")?;
        Ok(Self {
            child,
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
            next_id: 0,
        })
    }

    fn handshake(&mut self) -> Result<()> {
        let init = self.call(
            "initialize",
            json!({
                "clientInfo": { "name": "experience-ui", "version": "0.0.0" },
                "capabilities": {}
            }),
        )?;
        let _ = init;
        self.send_notification("initialized")?;
        Ok(())
    }

    fn send_notification(&mut self, method: &str) -> Result<()> {
        let line = serde_json::to_string(&json!({ "method": method }))?;
        writeln!(self.stdin, "{line}")?;
        self.stdin.flush()?;
        Ok(())
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let line = serde_json::to_string(&json!({
            "method": method,
            "id": id,
            "params": params,
        }))?;
        writeln!(self.stdin, "{line}")?;
        self.stdin.flush()?;
        loop {
            let mut buffer = String::new();
            let read = self.stdout.read_line(&mut buffer)?;
            if read == 0 {
                return Err(anyhow::anyhow!("app-server closed stdout"));
            }
            let message: Value = serde_json::from_str(buffer.trim())?;
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(result) = message.get("result") {
                    return Ok(result.clone());
                }
                if let Some(error) = message.get("error") {
                    return Err(anyhow::anyhow!("app-server error: {error}"));
                }
            }
        }
    }
}

impl Drop for StdioRpc {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

const PAGE: &str = r#"<!doctype html>
<html lang="zh">
<head>
<meta charset="utf-8"><title>Experience 管理</title>
<style>
body{font-family:system-ui;margin:24px;background:#0f1115;color:#e6e6e6}
table{border-collapse:collapse;width:100%;font-size:13px}
th,td{padding:6px 8px;border-bottom:1px solid #2a2e36;text-align:left}
tr{cursor:pointer}.row:hover{background:#1b2029}
.tag{padding:1px 7px;border-radius:9px;font-size:12px}
.Active{background:#0d4d24}.Disabled{background:#4d1c1c}.Candidate{background:#4d3f0d}.Decaying{background:#4d2a0d}
#detail{margin-top:18px;background:#151a22;padding:14px;border-radius:8px;display:none;white-space:pre-wrap;font-size:12px}
button{margin:4px 6px 0 0;background:#2b3442;color:#fff;border:0;border-radius:6px;padding:6px 12px;cursor:pointer}
</style></head>
<body>
<h2>Experience 管理</h2>
<button onclick="showImport()">新增（导入 JSON）</button>
<div id="importPanel" style="display:none">
  <textarea id="importJson" rows="6" cols="100" placeholder="粘贴 Experience JSON"></textarea><br>
  <button onclick="doImport()">导入</button>
</div>
<div id="status">加载中…</div>
<table id="rows"></table>
<div id="detail"></div>
<script>
async function api(path, opts){const r=await fetch(path,opts);return r.json();}
function esc(s){return String(s??'').replace(/[&<>"]/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c]));}
function disp(r){const t=r.title||r.name||'';return t.length>64?t.slice(0,64)+'…':t;}
function when(ts){if(!ts)return '未知';return new Date(ts*1000).toLocaleString();}
async function load(){
  const d=await api('/api/list');const rows=d.experiences||[];
  document.getElementById('status').textContent=rows.length+' 条经验';
  const t=document.getElementById('rows');
  t.innerHTML='<tr><th>标题</th><th>状态</th><th>置信度</th><th>生成时间</th><th>scope</th><th>pin</th><th>命中</th><th>misfire</th></tr>';
  for(const r of rows){const tr=document.createElement('tr');tr.className='row';
    tr.onclick=()=>show(r.id);
    tr.innerHTML=`<td>${esc(disp(r))}</td><td><span class="tag ${esc(r.status)}">${esc(r.status)}</span></td>
    <td>${r.confidence?.toFixed?.(2)??r.confidence}</td><td>${when(r.createdAt)}</td><td>${esc(r.inputScope)}</td>
    <td>${r.pinned?'📌':''}</td><td>${r.hits??0}</td><td>${r.misfires??0}</td>`;
    t.appendChild(tr);}
}
async function show(id){
  const d=await api('/api/detail?id='+encodeURIComponent(id));
  const el=document.getElementById('detail');el.style.display='block';
  el.innerHTML='<h3>'+esc(d.experience.name)+' <small>'+esc(d.experience.id)+'</small></h3>'+
    '<div>'+JSON.stringify(d,null,2)+'</div>'+
    '<button onclick="act(\''+esc(id)+'\',\'activate\')">activate</button>'+
    '<button onclick="act(\''+esc(id)+'\',\'revalidate\')">revalidate</button>'+
    '<button onclick="act(\''+esc(id)+'\',\'disable\')">disable</button>'+
    '<button onclick="act(\''+esc(id)+'\',\'pin\')">pin</button>'+
    '<button onclick="act(\''+esc(id)+'\',\'unpin\')">unpin</button>'+
    '<button onclick="act(\''+esc(id)+'\',\'delete\')">delete</button>'+
    '<hr><b>基本信息编辑</b><br>title: <input id="m_title"><br>'+
    'note: <textarea id="m_note" rows="2" cols="60"></textarea><br>'+
    'confidence: <input id="m_conf" type="number" step="0.01" min="0" max="1"><br>'+
    '<button onclick="saveMeta(\''+esc(id)+'\')">保存元数据</button>'+
    '<hr><b>state/runtime 作用编辑（applicability 与 conditions 各留 JSON，留空不变）</b><br>'+
    'applicability JSON: <input id="c_app" size="80" value="'+esc(JSON.stringify(d.experience.applicability||{}))+'"><br>'+
    'conditions JSON: <input id="c_cond" size="80" value="'+esc(JSON.stringify(d.experience.conditions||[]))+'"><br>'+
    '<button onclick="saveControl(\''+esc(id)+'\')">保存作用</button>'+
    '<hr><b>引用日志</b><br><pre>'+esc(JSON.stringify((d.usage&&d.usage.logs)||[],null,1))+'</pre>';
}
async function act(id,action){
  await api('/api/action',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({id,action})});
  await load();document.getElementById('detail').style.display='none';
}
async function saveMeta(id){
  await api('/api/meta',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({
    id,title:document.getElementById('m_title').value,note:document.getElementById('m_note').value,
    confidence:parseFloat(document.getElementById('m_conf').value)||null})});
  await load();await show(id);
}
async function saveControl(id){
  const appTxt=document.getElementById('c_app').value.trim();
  const condTxt=document.getElementById('c_cond').value.trim();
  await api('/api/control',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({
    id,applicability:appTxt?JSON.parse(appTxt):null,conditions:condTxt?JSON.parse(condTxt):null})});
  await load();await show(id);
}
async function showImport(){
  const el=document.getElementById('importPanel');
  el.style.display=el.style.display==='none'?'block':'none';
}
async function doImport(){
  const json=document.getElementById('importJson').value;
  const r=await api('/api/import',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({json})});
  document.getElementById('importPanel').style.display='none';await load();
}
load();
</script></body></html>"#;
