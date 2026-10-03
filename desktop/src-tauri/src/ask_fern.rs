//! Ask Noki as a remote-controlled window in the Miniatur.
//!
//! Ask is Noki's OWN window. The generic remote path (AX element at point,
//! PID-posted events, app activation) is wrong for it: activating Noki can
//! make macOS follow Ask to its Space (the user would be moved), a raise
//! would change Ask's z-order, and PID-posted keys to Noki would come back
//! through Noki's own key tap. So input for exactly this window goes
//! straight into its web view - the same DOM the user sees, nothing else:
//!   * no app activation, no key-window change, no Space change, no raise;
//!   * pointer/scroll coordinates are the real window's (helper-mapped
//!     global points minus the live content origin);
//!   * keys reach Ask only while a Miniatur click focused an editable
//!     element of Ask (fern_tippen's Ask session), never any other target.
//!
//! Typed characters are never logged - only counts and reasons.

use std::sync::Mutex;

/// Is this window the visible Ask Noki window (the shared inventory rule)?
pub fn ist_ask(wid: i64) -> bool {
    wid > 0 && crate::ask_nutzerfenster() == Some(wid)
}

/// Global point (helper-mapped, points) -> CSS px inside Ask's web view.
fn innen(app: &tauri::AppHandle, x: f64, y: f64) -> Option<(f64, f64)> {
    let win = crate::ask_fenster(app)?;
    let k = win.scale_factor().ok()?;
    let p = win.inner_position().ok()?.to_logical::<f64>(k);
    let g = win.inner_size().ok()?.to_logical::<f64>(k);
    let (cx, cy) = (x - p.x, y - p.y);
    (cx >= 0.0 && cy >= 0.0 && cx <= g.width && cy <= g.height).then_some((cx, cy))
}

fn ausfuehren(app: &tauri::AppHandle, js: String) -> bool {
    match crate::ask_fenster(app) {
        Some(w) => w.eval(&js).is_ok(),
        None => false,
    }
}

static APP: std::sync::OnceLock<tauri::AppHandle> = std::sync::OnceLock::new();
fn app_handle() -> Option<tauri::AppHandle> { APP.get().cloned() }

/// One pointer message from the Miniatur for the Ask window.
pub fn zeiger(app: &tauri::AppHandle, was: &str, wid: i64, x: f64, y: f64, teile: &[&str]) {
    let _ = APP.set(app.clone());
    let zahl = |i: usize| teile.get(i).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    match was {
        "klick" => klick(app, wid, x, y, (zahl(5) as i64).clamp(1, 3)),
        "rad" => rollen(app, x, y, zahl(5) * 40.0, zahl(6) * 40.0),
        "radf" => {
            // Helper units: precise = finger points * 0.25; "w" = wheel lines.
            let f = if teile.get(7) == Some(&"w") { 40.0 } else { 4.0 };
            rollen(app, x, y, zahl(5) * f, zahl(6) * f);
        }
        // Hover, right click and drags have no Ask route: consumed, no effect.
        _ => {}
    }
}

fn klick(app: &tauri::AppHandle, wid: i64, x: f64, y: f64, n: i64) {
    let Some((cx, cy)) = innen(app, x, y) else {
        crate::virtual_workspace::trace(&format!("[ASK_FERN] click wid={wid} outside_content=true"));
        return;
    };
    // A click elsewhere in Ask ends a running Ask typing session; the page
    // reports below whether this one focused an editable element.
    crate::fern_tippen::ask_beenden("remote_click");
    let js = format!(r#"(()=>{{const x={cx:.1},y={cy:.1},n={n};
const el=document.elementFromPoint(x,y); let ed=null;
if(el){{
 const o={{bubbles:true,cancelable:true,composed:true,clientX:x,clientY:y,button:0,buttons:1,detail:n,view:window}};
 const P=window.PointerEvent||MouseEvent, p={{pointerId:1,pointerType:'mouse',isPrimary:true}};
 el.dispatchEvent(new P('pointerdown',Object.assign({{}},o,p)));
 const frei=el.dispatchEvent(new MouseEvent('mousedown',o));
 ed=el.closest('textarea,input:not([type=button]):not([type=submit]):not([type=checkbox]):not([type=radio]):not([type=range]):not([type=file]),[contenteditable=""],[contenteditable="true"],[contenteditable="plaintext-only"]');
 if(frei){{
  if(ed){{
   const neu=document.activeElement!==ed; ed.focus({{preventScroll:true}});
   if(ed.isContentEditable&&document.caretRangeFromPoint){{const r=document.caretRangeFromPoint(x,y);if(r){{const s=getSelection();s.removeAllRanges();s.addRange(r);}}}}
   else if(neu&&ed.setSelectionRange){{try{{const l=ed.value.length;ed.setSelectionRange(l,l);}}catch(e){{}}}}
  }} else {{
   const f=el.closest('button,a[href],select,[tabindex]');
   if(f) f.focus({{preventScroll:true}}); else if(document.activeElement&&document.activeElement!==document.body) document.activeElement.blur();
  }}
 }}
 const u=Object.assign({{}},o,{{buttons:0}});
 el.dispatchEvent(new P('pointerup',Object.assign({{}},u,p)));
 el.dispatchEvent(new MouseEvent('mouseup',u));
 el.dispatchEvent(new MouseEvent('click',u));
 if(n===2) el.dispatchEvent(new MouseEvent('dblclick',u));
 // The page may have focused a field itself (pill/wrapper handlers); and
 // a click on a small wrapper around exactly ONE field means that field
 // (1 Miniatur point is ~2 real points - the thin field is easy to miss).
 const sel='textarea,input:not([type=button]):not([type=submit]):not([type=checkbox]):not([type=radio]):not([type=range]):not([type=file]),[contenteditable=""],[contenteditable="true"],[contenteditable="plaintext-only"]';
 const a=document.activeElement;
 if(!ed&&a&&a!==document.body&&a.matches&&a.matches(sel)) ed=a;
 if(!ed&&frei){{const k=el.querySelectorAll(sel), r=el.getBoundingClientRect();
  if(k.length===1&&r.height<160){{ed=k[0];ed.focus({{preventScroll:true}});}}}}
}}
try{{window.__TAURI__.core.invoke('ask_fern_feld',{{wid:{wid},editierbar:!!ed,treffer:(el?el.tagName+(el.className?'.'+String(el.className).split(' ')[0]:''):'-')+' '+x+','+y+' view='+innerWidth+'x'+innerHeight}});}}catch(e){{}}
}})()"#);
    let ok = ausfuehren(app, js);
    crate::vorschau::lebhaft(wid);
    crate::virtual_workspace::trace(&format!(
        "[ASK_FERN] click wid={wid} clicks={n} delivered={ok} route=ask_webview activation=false space_trip=false"));
}

/// Scroll packets arrive at 60-120/s: summed and delivered at most every
/// 16 ms (latest point wins, no queue).
static ROLLEN: Mutex<Option<(f64, f64, f64, f64)>> = Mutex::new(None);

fn rollen(app: &tauri::AppHandle, x: f64, y: f64, fdx: f64, fdy: f64) {
    if fdx == 0.0 && fdy == 0.0 { return; }
    let neu = match ROLLEN.lock() {
        Ok(mut g) => match g.as_mut() {
            Some(a) => { a.0 = x; a.1 = y; a.2 += fdx; a.3 += fdy; false }
            None => { *g = Some((x, y, fdx, fdy)); true }
        },
        Err(_) => return,
    };
    if !neu { return; }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(16));
        let Some((x, y, fdx, fdy)) = ROLLEN.lock().ok().and_then(|mut g| g.take()) else { return };
        let Some((cx, cy)) = innen(&app, x, y) else { return };
        // macOS scrollingDelta > 0 = towards the top; DOM deltaY > 0 = down.
        let (dx, dy) = (-fdx, -fdy);
        let js = format!(r#"(()=>{{const x={cx:.1},y={cy:.1},dx={dx:.2},dy={dy:.2};
const el=document.elementFromPoint(x,y)||document.body;
const w=new WheelEvent('wheel',{{bubbles:true,cancelable:true,composed:true,clientX:x,clientY:y,deltaX:dx,deltaY:dy,deltaMode:0}});
if(!el.dispatchEvent(w)) return;
const kann=(e,ax)=>{{const s=getComputedStyle(e),o=ax?s.overflowY:s.overflowX;return /(auto|scroll|overlay)/.test(o)&&(ax?e.scrollHeight>e.clientHeight+1:e.scrollWidth>e.clientWidth+1);}};
const ziel=ax=>{{for(let e=el;e&&e!==document.documentElement;e=e.parentElement){{if(kann(e,ax))return e;}}return document.scrollingElement;}};
if(dy) ziel(true).scrollBy({{top:dy,behavior:'instant'}});
if(dx) ziel(false).scrollBy({{left:dx,behavior:'instant'}});
}})()"#);
        let _ = ausfuehren(&app, js);
        if let Some(wid) = crate::ask_nutzerfenster() { crate::vorschau::lebhaft(wid); }
    });
}

/// Page answer to a remote click: did it focus an editable element of Ask?
#[tauri::command]
pub fn ask_fern_feld(wid: i64, editierbar: bool, treffer: Option<String>) {
    crate::virtual_workspace::trace(&format!(
        "[ASK_FERN] page_hit wid={wid} editable={editierbar} hit={}", treffer.unwrap_or_default()));
    if editierbar && ist_ask(wid) && crate::vorschau::sichtbar() {
        crate::fern_tippen::beginnen_ask(wid);
    }
}

/// Clipboard copy requested by the page (Cmd+C / Cmd+X in an Ask session).
#[tauri::command]
pub fn ask_fern_kopieren(text: String) {
    std::thread::spawn(move || {
        use std::io::Write;
        if let Ok(mut k) = std::process::Command::new("/usr/bin/pbcopy")
            .stdin(std::process::Stdio::piped()).spawn()
        {
            if let Some(mut ein) = k.stdin.take() { let _ = ein.write_all(text.as_bytes()); }
            let _ = k.wait();
        }
    });
}

/// One key of an Ask typing session into the focused element of Ask's page.
/// `name` is the DOM key name ("Backspace", "Enter", "a", ...), `text` the
/// characters to insert (empty for non-text keys).
pub fn taste(name: &str, text: &str, shift: bool, alt: bool) {
    let Some(app) = app_handle() else { return };
    let (k, t) = (serde_json::to_string(name).unwrap_or_default(), serde_json::to_string(text).unwrap_or_default());
    let js = format!(r#"(()=>{{const k={k},t={t},sh={shift},alt={alt};
const el=document.activeElement||document.body;
const o={{key:k,bubbles:true,cancelable:true,composed:true,shiftKey:sh,altKey:alt}};
if(!el.dispatchEvent(new KeyboardEvent('keydown',o))){{el.dispatchEvent(new KeyboardEvent('keyup',o));return;}}
const feld=el.tagName==='TEXTAREA'||el.tagName==='INPUT', ed=feld||el.isContentEditable;
if(t){{ if(ed) document.execCommand('insertText',false,t); }}
else if(k==='Backspace'){{ if(ed) document.execCommand('delete'); }}
else if(k==='Delete'){{ if(ed) document.execCommand('forwardDelete'); }}
else if(k==='Enter'){{ if(el.tagName==='TEXTAREA'||el.isContentEditable) document.execCommand('insertLineBreak'); else if(el.form&&el.form.requestSubmit) el.form.requestSubmit(); }}
else if(k.startsWith('Arrow')||k==='Home'||k==='End'){{
 const zur=k==='ArrowLeft'||k==='ArrowUp'||k==='Home', g=k==='ArrowUp'||k==='ArrowDown'?'line':(k==='Home'||k==='End'?'lineboundary':'character');
 if(feld&&g!=='line'&&el.setSelectionRange){{
  const a=el.selectionStart,b=el.selectionEnd,l=el.value.length;
  let p=g==='lineboundary'?(zur?0:l):(a!==b&&!sh?(zur?a:b):Math.max(0,Math.min(l,(zur?a:b)+(zur?-1:1))));
  if(sh) el.setSelectionRange(Math.min(a,p),Math.max(b,p)); else el.setSelectionRange(p,p);
 }} else {{ try{{getSelection().modify(sh?'extend':'move',zur?'backward':'forward',g);}}catch(e){{}} }}
}}
el.dispatchEvent(new KeyboardEvent('keyup',o));
}})()"#);
    let _ = ausfuehren(&app, js);
    if let Some(wid) = crate::ask_nutzerfenster() { crate::vorschau::lebhaft(wid); }
}

/// Cmd shortcuts of an Ask typing session: a, c, x, v, z (shift = redo).
pub fn befehl(z: char, shift: bool) {
    let Some(app) = app_handle() else { return };
    if z == 'v' {
        // Paste = insert the clipboard's plain text (never a local Cmd+V).
        std::thread::spawn(move || {
            let text = std::process::Command::new("/usr/bin/pbpaste").output()
                .ok().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
            if !text.is_empty() { taste("v", &text, false, false); }
        });
        return;
    }
    let js = match z {
        'a' => "(()=>{const el=document.activeElement;if(el&&el.select&&(el.tagName==='TEXTAREA'||el.tagName==='INPUT'))el.select();else document.execCommand('selectAll');})()".to_string(),
        'z' => format!("document.execCommand('{}')", if shift { "redo" } else { "undo" }),
        'c' | 'x' => format!(r#"(()=>{{const el=document.activeElement;let t='';
if(el&&(el.tagName==='TEXTAREA'||el.tagName==='INPUT')&&el.selectionStart!=null)t=el.value.substring(el.selectionStart,el.selectionEnd);else t=String(getSelection());
if(t){{try{{window.__TAURI__.core.invoke('ask_fern_kopieren',{{text:t}});}}catch(e){{}}{}}}}})()"#,
            if z == 'x' { "document.execCommand('delete');" } else { "" }),
        _ => return,
    };
    let _ = ausfuehren(&app, js);
    if let Some(wid) = crate::ask_nutzerfenster() { crate::vorschau::lebhaft(wid); }
}
