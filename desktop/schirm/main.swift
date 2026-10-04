// nokischirm — Nokis Bildschirm-Fenster fuer die Arbeitsplatz-Vorschau.
//
// Warum ein eigener kleiner Prozess und keine Rust-Bindung: ScreenCaptureKit
// ist eine Swift-/ObjC-API mit Delegates und Nebenlaeufigkeit. Noki startet
// ohnehin schon Fachprozesse (specialist.rs) - das hier ist einer davon.
//
// Gemessen auf macOS 26.6: ScreenCaptureKit liefert ECHTE, LAUFENDE Bilder
// von Fenstern auf einem INAKTIVEN Space. Ob sich der Inhalt aendert,
// entscheidet die App selbst: Chrome drosselt sein Zeichnen, wenn sein
// Fenster verdeckt ist.
//
// DIESER Prozess zeigt die Miniatur auch selbst an - in einem eigenen,
// nicht aktivierenden Fenster. Zwei Gruende, beide gemessen:
//
//  * Im wandernden Figuren-Overlay rutschte die Miniatur bei jedem
//    Schreibtischwechsel mit hinaus, fehlte dann bis zum Umzug der Figur
//    und blitzte beim Umzug auf Nokis Schreibtisch ein Bild lang auf
//    (Aufnahme v9, Bild 65). Dieses Fenster ist dagegen Mitglied JEDES
//    Nutzer-Schreibtischs und NIE von Nokis eigenem: es ist auf jedem
//    Nutzer-Schreibtisch schon da, bevor er zu sehen ist, und auf Nokis
//    Schreibtisch gar nicht erst vorhanden. Kein Rennen, keine Wartezeit.
//  * Bilder gehen nicht mehr als JPEG/Base64 durch das WebView, sondern die
//    IOSurface von ScreenCaptureKit wird direkt als Ebeneninhalt gezeigt.
//    ScreenCaptureKit liefert nur bei Aenderung ein Bild: ein stiller
//    Schreibtisch kostet fast nichts, ein Video laeuft bis 30 Bilder/s.
//
// Protokoll (stdout):  META <id> <x> <y> <w> <h> <app>    DESK <x> <y> <w> <h>
//                      WALL <bytes>\n<JPEG>   GONE <id>   DEAD <id>
//                      READY <anzahl>   ERR <text>   KLICK <besuchen|gross|zu>
//                      ABGEDECKT   AUFGEDECKT   FPS <bilder/s>
// Protokoll (stdin):   set <id,...>   pause   resume   quit
//                      rahmen <x> <y> <w> <h>   (Bildschirmpunkte, oben links; w=0 verbirgt)
//                      label <text>   modus <gross|kompakt>   spaces <sid,...>
//                      abdecken   aufdecken <sid,...>   fern <an|aus>

import Foundation
import ScreenCaptureKit
import CoreImage
import AppKit
import QuartzCore
import IOSurface

let out = FileHandle.standardOutput
let schreibSperre = NSLock()

func sendeText(_ s: String) {
    schreibSperre.lock(); defer { schreibSperre.unlock() }
    out.write(Data((s + "\n").utf8))
}

let FUSS: CGFloat = 24

/// Groesse des Schreibtischs. Bezugssystem ist das der Fensterrahmen
/// (Ursprung links OBEN), damit sich beides ohne Umrechnung ineinander legt.
var schreibtisch = CGDisplayBounds(CGMainDisplayID())
/// Die Anzeige, deren Flaeche `schreibtisch` ist (0 = unbekannt). Mit ihr
/// liest der Helfer die Grenzen nach einer Bildschirm-Neuordnung selbst
/// frisch, statt einem alten Wert aus Noki zu vertrauen.
var schreibtischAnzeige: CGDirectDisplayID = 0
func sendeSchreibtisch() {
    let b = schreibtisch
    sendeText("DESK \(Int(b.origin.x)) \(Int(b.origin.y)) \(Int(b.width)) \(Int(b.height))")
}

/// Das echte Schreibtischbild - fuer die eigene Anzeige und, als JPEG, fuer
/// die Oberflaeche (Seitenverhaeltnis, Rueckfall ohne Fenster).
func ladeHintergrund() -> CGImage? {
    guard let screen = NSScreen.main,
          let url = NSWorkspace.shared.desktopImageURL(for: screen),
          let bild = CIImage(contentsOf: url) else { sendeText("WALLNONE"); return nil }
    let ci = CIContext()
    let ziel: CGFloat = 1600
    let s = min(1.0, ziel / max(1.0, bild.extent.width))
    let klein = bild.transformed(by: CGAffineTransform(scaleX: s, y: s))
    if let jpeg = ci.jpegRepresentation(
        of: klein, colorSpace: CGColorSpaceCreateDeviceRGB(),
        options: [kCGImageDestinationLossyCompressionQuality as CIImageRepresentationOption: 0.7]) {
        schreibSperre.lock()
        out.write(Data("WALL \(jpeg.count)\n".utf8))
        out.write(jpeg)
        schreibSperre.unlock()
    }
    return ci.createCGImage(klein, from: klein.extent)
}

// ---------------------------------------------------------------------------
//  WindowServer-Hilfen (privat, per dlsym; fehlt ein Symbol, passiert nichts)
// ---------------------------------------------------------------------------
let cgsGriff = dlopen("/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics", RTLD_NOW)
typealias CgsVerbindung = @convention(c) () -> Int32
typealias CgsSpacesFuerFenster = @convention(c) (Int32, Int32, CFArray) -> Unmanaged<CFArray>?
typealias CgsFensterSpaces = @convention(c) (Int32, CFArray, CFArray) -> Void
typealias CgFensterBild = @convention(c) (
    CGRect, CGWindowListOption, CGWindowID, CGWindowImageOption
) -> Unmanaged<CGImage>?
let cgsCid: Int32 = {
    guard let a = dlsym(cgsGriff, "CGSMainConnectionID") else { return 0 }
    return unsafeBitCast(a, to: CgsVerbindung.self)()
}()

/// macOS 15 marks this API unavailable at compile time, but WindowServer
/// still exports it on the supported machines.  It is only a bounded
/// fallback when ScreenCaptureKit supplies no initial sample at all.
func fensterStandbild(_ id: UInt32, nominal: Bool = false) -> CGImage? {
    guard let p = dlsym(cgsGriff, "CGWindowListCreateImage") else { return nil }
    // `nominal`: point resolution (1 px/pt) - the first picture of a target
    // switch. The Miniatur shows a window at ~0.48 px/pt (compact) and
    // ~0.96 px/pt (hover), so it is still >= 1:1, at a quarter of the
    // WindowServer work (measured: 10 full-Retina snapshots 595 ms). The
    // live stream replaces it at full sharpness right after.
    return unsafeBitCast(p, to: CgFensterBild.self)(
        .null, .optionIncludingWindow, CGWindowID(id),
        nominal ? [.boundsIgnoreFraming, .nominalResolution] : [.boundsIgnoreFraming, .bestResolution]
    )?.takeRetainedValue()
}

func spacesVon(_ id: UInt32) -> [UInt64] {
    guard let b = dlsym(cgsGriff, "CGSCopySpacesForWindows") else { return [] }
    let sp = unsafeBitCast(b, to: CgsSpacesFuerFenster.self)(cgsCid, 7, [NSNumber(value: id)] as CFArray)?
        .takeRetainedValue() as? [NSNumber] ?? []
    return sp.map { $0.uint64Value }
}

func fensterSpaces(_ name: String, _ id: Int, _ sids: [UInt64]) {
    guard !sids.isEmpty, let p = dlsym(cgsGriff, name) else { return }
    unsafeBitCast(p, to: CgsFensterSpaces.self)(
        cgsCid, [NSNumber(value: id)] as CFArray, sids.map { NSNumber(value: $0) } as CFArray)
}

/// Steht dieses Fenster noch auf IRGENDEINEM Space? "Existiert" reicht
/// nicht: Finder behaelt ein geschlossenes Fenster bei WindowServer, nur
/// ohne Space (gemessen, Fenster 384914) - sichtbar ist es nirgends mehr.
/// Bewusst die volle Liste: `.optionIncludingWindow` liefert fuer manche
/// lebenden, verborgenen Fenster nichts (gemessen: Chrome-Popup 321066).
func fensterLebt(_ id: UInt32) -> Bool {
    let l = CGWindowListCopyWindowInfo([], kCGNullWindowID) as? [[String: Any]] ?? []
    guard l.contains(where: { ($0[kCGWindowNumber as String] as? NSNumber)?.uint32Value == id }) else {
        return false
    }
    return !spacesVon(id).isEmpty
}

/// Stapelreihenfolge von WindowServer (vorn zuerst) -> Ebenen-z.
func stapel() -> [UInt32: Int] {
    // `.optionOnScreenOnly` ist hier wesentlich: OHNE Option liefert
    // CGWindowList zwar alle Fenster, aber in einer Reihenfolge, die NICHTS
    // ueber vorn und hinten aussagt. Gemessen stand YouTube dort auf Platz
    // 11 und Claude auf 43, waehrend auf dem Schreibtisch sichtbar Claude
    // oben lag - daran hing die falsche Stapelung in der Miniatur.
    // Mit der Option ist die Liste die echte Reihenfolge von vorn nach
    // hinten, allerdings nur fuer den GERADE sichtbaren Schreibtisch.
    let l = CGWindowListCopyWindowInfo(.optionOnScreenOnly, kCGNullWindowID) as? [[String: Any]] ?? []
    var z: [UInt32: Int] = [:]
    for (i, w) in l.enumerated() {
        if let n = (w[kCGWindowNumber as String] as? NSNumber)?.uint32Value { z[n] = l.count - i }
    }
    // REAL_SPACE: Noki's windows live on a HIDDEN Space and are never
    // "on screen" - the list above never contained them, so the Miniatur
    // had no native order at all (stale/default z). WindowServer's per-Space
    // list is the real front-to-back order of that Space, and it DOES follow
    // an AXRaise of an inactive app's window (measured 2026-09-26).
    let reihe = spaceReihe(nokiSpace)
    for (i, n) in reihe.enumerated() { z[n] = 100_000 + reihe.count - i }
    return z
}

/// Front-to-back window order of ONE Space (WindowServer's own list).
typealias SpaceFensterF = @convention(c) (Int32, Int32, CFArray, Int32, UnsafeMutablePointer<UInt64>, UnsafeMutablePointer<UInt64>) -> Unmanaged<CFArray>?
let spaceFensterF: SpaceFensterF? = cgsGriff.flatMap { dlsym($0, "CGSCopyWindowsWithOptionsAndTags") }.map { unsafeBitCast($0, to: SpaceFensterF.self) }
func spaceReihe(_ sid: UInt64) -> [UInt32] {
    guard sid != 0, let f = spaceFensterF else { return [] }
    var setzen: UInt64 = 0, loeschen: UInt64 = 0
    guard let a = f(cgsCid, 0, [NSNumber(value: sid)] as CFArray, 2, &setzen, &loeschen)?.takeRetainedValue() as? [NSNumber] else { return [] }
    return a.map { $0.uint32Value }
}

// ---------------------------------------------------------------------------
//  Die Anzeige
// ---------------------------------------------------------------------------
final class Knopf: NSView {
    var symbol: String { didSet { needsDisplay = true } }
    init(_ s: String) { symbol = s; super.init(frame: .zero) }
    required init?(coder: NSCoder) { fatalError() }
    override func draw(_ r: NSRect) {
        guard let roh = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)?
            .withSymbolConfiguration(.init(pointSize: 9.5, weight: .semibold)) else { return }
        let s = roh.size
        let img = NSImage(size: s, flipped: false) { r in
            roh.draw(in: r)
            NSColor(white: 0.94, alpha: 0.7).set()
            r.fill(using: .sourceAtop)
            return true
        }
        img.draw(in: NSRect(x: (bounds.width - s.width) / 2, y: (bounds.height - s.height) / 2,
                            width: s.width, height: s.height))
    }
}

/// "Zum Schreibtisch N" - der EINZIGE Klickweg, der den Nutzer noch bewegt.
///
/// Die Schreibtischflaeche darueber ist ab jetzt Inhalt, den man bedient;
/// Navigation braucht deshalb eine eigene, sichtbare Stelle. Die Nummer wird
/// gesetzt, nie geraten: sie kommt aus der laufenden Mission-Control-Ordnung.
/// Menue-Handlung als Abschluss (NSMenu braucht target/action).
final class MenuZiel: NSObject {
    let aktion: () -> Void
    init(_ a: @escaping () -> Void) { aktion = a }
    @objc func los() { aktion() }
}

/// Waehrend ein Menue der Leiste offen ist, gehoert die Groesse nicht dem Zeiger.
var menueOffen = false

func menuePunkt(_ titel: String, bild: NSImage? = nil, an: Bool = false, _ a: @escaping () -> Void) -> NSMenuItem {
    let z = MenuZiel(a)
    let i = NSMenuItem(title: titel, action: #selector(MenuZiel.los), keyEquivalent: "")
    i.target = z
    i.representedObject = z
    i.state = an ? .on : .off
    if let b = bild { b.size = NSSize(width: 16, height: 16); i.image = b }
    return i
}

/// Kompakte Geometrie-Spur - nur wenn sich die Lage aendert (kein Dauerlog).
var spurStand: [UInt32: String] = [:]
@MainActor func geometrieSpur(_ id: UInt32, puffer: CGSize, inhalt: CGRect, grund: String = "frame") {
    let o = ansicht.orte[id] ?? .zero
    let skala = aufnahmeSkala()
    let soll = CGSize(width: (o.width * skala).rounded(), height: (o.height * skala).rounded())
    let passt = abs(inhalt.width - soll.width) <= 3 && abs(inhalt.height - soll.height) <= 3
    let k = "\(Int(o.width))x\(Int(o.height)) \(Int(puffer.width))x\(Int(puffer.height)) \(Int(inhalt.width))x\(Int(inhalt.height)) \(passt)"
    guard spurStand[id] != k else { return }
    spurStand[id] = k
    let l = ansicht.fensterEbenen[id]?.frame ?? .zero
    sendeText("FRESH WIN \(id) \(grund) framePoints=\(Int(o.minX)),\(Int(o.minY)),\(Int(o.width))x\(Int(o.height)) sourcePixels=\(Int(puffer.width))x\(Int(puffer.height)) contentRect=\(Int(inhalt.minX)),\(Int(inhalt.minY)),\(Int(inhalt.width))x\(Int(inhalt.height)) destRect=\(Int(l.minX)),\(Int(l.minY)),\(Int(l.width))x\(Int(l.height)) scale=\(String(format: "%.3f", skala)) srcPerDestPx=\(String(format: "%.2f", l.width > 0 ? inhalt.width / (l.width * (NSScreen.main?.backingScaleFactor ?? 2)) : 0)) sharp=\(passt)")
    // Inhalt passt nicht zur Fenstergroesse: Strom neu vermessen (gedrosselt).
    // Nach einem Hover-Wechsel ist die Abweichung gewollt (entprellte
    // Hoch-/Herunterstufung) - nicht vorzeitig neu vermessen.
    if !passt && Date().timeIntervalSince(hoverZeit) > 8.5 { vermessenPlanen(grund: "content_mismatch") }
}

/// Strom-Neuvermessung, gedrosselt: hoechstens alle 0,3 s, und immer eine
/// letzte nach dem Ende einer Aenderung.
var vermessenLetzt = Date.distantPast
var vermessenNachlauf: DispatchWorkItem?
var vermessenGeplant = false
@MainActor func vermessenPlanen(grund: String) {
    // Drosseln, nicht entprellen: laeuft schon eine geplante Vermessung,
    // bleibt sie stehen - so wird auch WAEHREND eines Resize alle 0,3 s scharf
    // nachgezogen, nicht erst am Ende.
    if let v = vermessenNachlauf, !v.isCancelled, vermessenGeplant { return }
    vermessenGeplant = true
    let item = DispatchWorkItem { vermessenGeplant = false; vermessenLetzt = Date(); Task { await aufnahme.neuVermessen() } }
    vermessenNachlauf = item
    let warte = max(0.12, 0.3 - Date().timeIntervalSince(vermessenLetzt))
    DispatchQueue.main.asyncAfter(deadline: .now() + warte, execute: item)
}

/// Programm nach vorn - auch aus dem Hintergrund. `NSApp.activate` wird bei
/// kooperativer Aktivierung (macOS 14+) fuer ein Hintergrundprogramm
/// ignoriert (gemessen: Tastendruecke gingen weiter an das Programm des
/// Nutzers und schlossen das Menue). SkyLight nimmt den Wunsch als
/// "vom Nutzer ausgeloest" an - wie Programmumschalter es tun.
func vordergrund(_ pid: pid_t) {
    typealias FPsn = @convention(c) (pid_t, UnsafeMutablePointer<ProcessSerialNumber>) -> OSStatus
    typealias FFront = @convention(c) (UnsafeMutablePointer<ProcessSerialNumber>, UInt32, UInt32) -> CGError
    let sky = dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight", RTLD_NOW)
    let alle = dlopen(nil, RTLD_NOW)
    if let fp = dlsym(alle, "GetProcessForPID"), let ff = dlsym(sky, "_SLPSSetFrontProcessWithOptions") {
        var psn = ProcessSerialNumber()
        if unsafeBitCast(fp, to: FPsn.self)(pid, &psn) == noErr {
            _ = unsafeBitCast(ff, to: FFront.self)(&psn, 0, 0x200) // kCPSUserGenerated
            return
        }
    }
    NSRunningApplication(processIdentifier: pid)?.activate(options: [])
}

/// EXACTLY this window of its app to the front (key + front), accepted as
/// user-initiated. Only used right after an explicit visit, when the window
/// is on the Space the user is now on - never across Spaces.
func fensterVordergrund(_ pid: pid_t, _ wid: UInt32) -> Bool {
    typealias FPsn = @convention(c) (pid_t, UnsafeMutablePointer<ProcessSerialNumber>) -> OSStatus
    typealias FFront = @convention(c) (UnsafeMutablePointer<ProcessSerialNumber>, UInt32, UInt32) -> CGError
    let sky = dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight", RTLD_NOW)
    let alle = dlopen(nil, RTLD_NOW)
    guard let fp = dlsym(alle, "GetProcessForPID"), let ff = dlsym(sky, "_SLPSSetFrontProcessWithOptions") else { return false }
    var psn = ProcessSerialNumber()
    guard unsafeBitCast(fp, to: FPsn.self)(pid, &psn) == noErr else { return false }
    return unsafeBitCast(ff, to: FFront.self)(&psn, wid, 0x200) == .success
}

func menueZeigen(_ m: NSMenu, an v: NSView) {
    menueOffen = true
    sendeText("LEISTENMENUE an")
    // Ein Menue eines nicht aktiven Hintergrundprogramms bekommt keine
    // Tastatur (das Suchfeld blieb stumm), und macOS gab den Vordergrund
    // danach an den Finder (gemessen). Deshalb: fuer die Dauer des Menues
    // selbst aktiv (der Nutzer hat eben geklickt), danach bekommt das
    // Programm des Nutzers den Vordergrund zurueck.
    let vorher = NSWorkspace.shared.frontmostApplication
    vordergrund(getpid())
    m.popUp(positioning: nil, at: NSPoint(x: 0, y: v.bounds.height + 4), in: v)
    // Nur zurueck an ein Programm, das auf DIESEM Schreibtisch ein Fenster
    // zeigt - sonst koennte macOS dafuer den Schreibtisch wechseln.
    if let vorher, vorher.processIdentifier != getpid(), !vorher.isTerminated {
        let sichtbar = (CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? [])
            .contains { ($0[kCGWindowOwnerPID as String] as? pid_t) == vorher.processIdentifier }
        if sichtbar { vordergrund(vorher.processIdentifier) }
    }
    menueOffen = false
    sendeText("LEISTENMENUE aus")
    DispatchQueue.main.async { hoverAbgleichen() }
}

/// Ein Programm der App-Leiste (oder "+" / "x"). Dieselbe Sprache wie der
/// Fussknopf: warmer Sandton, feiner Rand, kein Blau.
final class LeistenKnopf: NSView {
    var bild: NSImage?
    var text = ""
    var name = ""
    var zahl = 0
    var aktiv = false { didSet { needsDisplay = true } }
    var drin = false { didSet { needsDisplay = true } }
    var klick: (() -> Void)?
    var rechts: (() -> Void)?
    var hilfe = "" { didSet { toolTip = hilfe } }
    var breite: CGFloat {
        let t = text.isEmpty ? 0 : (text as NSString).size(withAttributes: [.font: NSFont.systemFont(ofSize: 9.5, weight: .medium)]).width + 4
        return (bild == nil ? 0 : 16) + t + (zahl > 1 ? 12 : 0) + 10
    }
    override func draw(_ r: NSRect) {
        let sand = NSColor(red: 0.89, green: 0.871, blue: 0.831, alpha: 1)
        let rund = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 6, yRadius: 6)
        sand.withAlphaComponent(aktiv ? 0.20 : (drin ? 0.12 : 0.04)).setFill(); rund.fill()
        sand.withAlphaComponent(aktiv ? 0.55 : (drin ? 0.36 : 0.16)).setStroke(); rund.lineWidth = 1; rund.stroke()
        var x: CGFloat = 5
        if let b = bild { b.draw(in: NSRect(x: x, y: (bounds.height - 14) / 2, width: 14, height: 14)); x += 16 }
        if !text.isEmpty {
            let f: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 9.5, weight: .medium),
                                                    .foregroundColor: sand.withAlphaComponent(aktiv || drin ? 1 : 0.8)]
            let w = (text as NSString).size(withAttributes: f).width
            (text as NSString).draw(at: NSPoint(x: x + 2, y: (bounds.height - 12) / 2), withAttributes: f)
            x += w + 4
        }
        if zahl > 1 {
            ("\(zahl)" as NSString).draw(at: NSPoint(x: x, y: (bounds.height - 11) / 2),
                withAttributes: [.font: NSFont.systemFont(ofSize: 8, weight: .semibold), .foregroundColor: sand.withAlphaComponent(0.7)])
        }
    }
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach { removeTrackingArea($0) }
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self, userInfo: nil))
    }
    override func mouseEntered(with e: NSEvent) { drin = true }
    override func mouseExited(with e: NSEvent) { drin = false }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    var gedrueckt = false
    override func mouseDown(with e: NSEvent) {
        gedrueckt = true
        sendeText("INTERACTION an")
    }
    override func mouseUp(with e: NSEvent) {
        defer { gedrueckt = false }
        // Die Leiste wird nach jedem Klick neu aufgebaut (Hervorhebung).
        // Kam der naechste Klick genau dazwischen, hing dieser Knopf beim
        // Loslassen nicht mehr im Fenster - die Pruefung schlug fehl und der
        // Klick verfiel (gemessen: 3 von 12). Gedrueckt ist gedrueckt.
        let losgelassenDrauf = window == nil ? gedrueckt : bounds.contains(convert(e.locationInWindow, from: nil))
        guard losgelassenDrauf else { return }
        if e.modifierFlags.contains(.control), let r = rechts { r() } else { klick?() }
    }
    override func rightMouseDown(with e: NSEvent) { }
    override func rightMouseUp(with e: NSEvent) { rechts?() }
}

final class NaviKnopf: NSView {
    var titel = "Zum Schreibtisch" { didSet { needsDisplay = true } }
    /// Der Winkel steht fuer "dorthin". Beim Schliessen der Vollansicht
    /// geht es nirgendwohin - dann ohne Winkel.
    var pfeil = true { didSet { needsDisplay = true } }
    var drin = false { didSet { needsDisplay = true } }
    var gedrueckt = false
    var breite: CGFloat {
        (titel as NSString).size(withAttributes: [.font: NSFont.systemFont(ofSize: 9.5, weight: .medium)]).width + 28
    }
    /// Dieselbe Sprache wie Nokis Einstellungen: warmer Sandton, ein Hauch
    /// Flaeche, feiner Rand, 6 pt Radius - und ein kleiner Pfeil, weil es um
    /// einen Ortswechsel geht. Kein blauer Knopf, kein Farbverlauf.
    override func draw(_ r: NSRect) {
        let sand = NSColor(red: 0.89, green: 0.871, blue: 0.831, alpha: 1)
        let rund = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 6, yRadius: 6)
        sand.withAlphaComponent(drin ? 0.16 : 0.08).setFill()
        rund.fill()
        sand.withAlphaComponent(drin ? 0.46 : 0.26).setStroke()
        rund.lineWidth = 1
        rund.stroke()
        let farbe = sand.withAlphaComponent(drin ? 1.0 : 0.82)
        let schrift = NSFont.systemFont(ofSize: 9.5, weight: .medium)
        let breiteText = (titel as NSString).size(withAttributes: [.font: schrift]).width
        let x0 = (bounds.width - (breiteText + (pfeil ? 11 : 0))) / 2
        (titel as NSString).draw(
            in: NSRect(x: x0, y: (bounds.height - 11) / 2, width: breiteText + 1, height: 12),
            withAttributes: [.font: schrift, .foregroundColor: farbe])
        // Ein schmaler Winkel nach rechts - dieselbe Strichstaerke wie die
        // uebrigen Zeichen im Fussband.
        guard pfeil else { return }
        let m = bounds.midY, px = x0 + breiteText + 5
        let pf = NSBezierPath()
        pf.move(to: NSPoint(x: px, y: m + 3))
        pf.line(to: NSPoint(x: px + 3.5, y: m))
        pf.line(to: NSPoint(x: px, y: m - 3))
        pf.lineWidth = 1.2
        pf.lineCapStyle = .round
        pf.lineJoinStyle = .round
        farbe.setStroke()
        pf.stroke()
    }
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach { removeTrackingArea($0) }
        addTrackingArea(NSTrackingArea(rect: bounds,
                                       options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
                                       owner: self, userInfo: nil))
    }
    override func mouseEntered(with e: NSEvent) { drin = true }
    override func mouseExited(with e: NSEvent) { drin = false }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func mouseDown(with e: NSEvent) {
        sendeText("INTERACTION an")
        gedrueckt = bounds.contains(convert(e.locationInWindow, from: nil))
        sendeText("KLICKART pointerDown region=footer hit=ENTER_SPACE_ACTION selectedSpaceID=\(nokiSpace) currentPhysicalSpaceID=\(aktiverSpaceID())")
    }
    override func mouseUp(with e: NSEvent) {
        let warGedrueckt = gedrueckt
        gedrueckt = false
        guard warGedrueckt,
              bounds.contains(convert(e.locationInWindow, from: nil)) else { return }
        // In der Vollansicht schliesst derselbe Knopf sie wieder.
        if vollAn { vollSetzen(false); return }
        sendeText("KLICKART action=ENTER_SPACE_ACTION selectedSpaceID=\(nokiSpace) currentPhysicalSpaceID=\(aktiverSpaceID())")
        // The ONLY message that may lead to a Space visit - and it says
        // where it came from (primary mouse down+up inside this button).
        sendeText("KLICK besuchen footer_primary")
    }
}

/// Farben der Noki-Oberflaeche (wie Noki Einstellungen): warmer Sandton als
/// Akzent, helle Schrift mit klarer Rangfolge, feine Linien. Die Apps-Liste
/// liegt auf einer dunklen, sandgetoenten und leicht durchscheinenden Flaeche
/// - nicht auf einem hellen Cremeblatt.
enum Ton {
    static let flaeche = NSColor(red: 0.290, green: 0.262, blue: 0.220, alpha: 0.78)   // dunkler Sand
    static let feld = NSColor(red: 1.0, green: 0.965, blue: 0.90, alpha: 0.075)
    static let feldRand = NSColor(red: 0.839, green: 0.808, blue: 0.69, alpha: 0.24)
    static let linie = NSColor(white: 1, alpha: 0.075)
    static let text = NSColor(red: 0.953, green: 0.945, blue: 0.925, alpha: 1)       // #f3f1ec
    static let text2 = NSColor(red: 0.925, green: 0.910, blue: 0.875, alpha: 0.66)
    static let sand = NSColor(red: 0.839, green: 0.808, blue: 0.69, alpha: 1)         // #d6ceb0
}

final class SandFlaeche: NSView {
    var linie = true
    override func draw(_ dirtyRect: NSRect) {
        if !AppZeile.fensterGetoent && linie { Ton.flaeche.setFill(); bounds.fill() }
        guard linie else { return }
        Ton.linie.setFill(); NSRect(x: 12, y: 0, width: bounds.width - 24, height: 1).fill()
    }
}

final class AppZeile: NSView {
    static var fensterGetoent = false
    let icon = NSImageView()
    let label = NSTextField(labelWithString: "")
    let status = NSTextField(labelWithString: "")
    var drin = false { didSet { needsDisplay = true } }
    var aktion: (() -> Void)?
    weak var trackingMenu: NSMenu?

    init(name: String, bild: NSImage?, status text: String) {
        super.init(frame: NSRect(x: 0, y: 0, width: 344, height: 46))
        icon.image = bild
        icon.imageScaling = .scaleProportionallyUpOrDown
        icon.frame = NSRect(x: 14, y: 9, width: 28, height: 28)
        label.stringValue = name
        label.font = .systemFont(ofSize: 13.5, weight: .semibold)
        label.textColor = Ton.text
        label.lineBreakMode = .byTruncatingTail
        status.stringValue = text
        status.font = .systemFont(ofSize: 11, weight: .regular)
        status.textColor = Ton.text2
        status.lineBreakMode = .byTruncatingTail
        if text.isEmpty {
            label.frame = NSRect(x: 52, y: 14, width: 276, height: 18)
        } else {
            label.frame = NSRect(x: 52, y: 22, width: 276, height: 18)
            status.frame = NSRect(x: 52, y: 7, width: 276, height: 14)
            addSubview(status)
        }
        addSubview(icon); addSubview(label)
    }
    required init?(coder: NSCoder) { fatalError() }
    override func draw(_ dirtyRect: NSRect) {
        // Grundflaeche liefert moeglichst das Menuefenster (einheitlicher
        // Sandton, auch unter weggefilterten Zeilen); sonst die Zeile selbst.
        if !AppZeile.fensterGetoent { Ton.flaeche.setFill(); bounds.fill() }
        guard drin else { return }
        // Hover: nur ein Hauch hellerer Sand, kein Rahmen, kein Leuchten.
        Ton.sand.withAlphaComponent(0.13).setFill()
        NSBezierPath(roundedRect: bounds.insetBy(dx: 6, dy: 2), xRadius: 7, yRadius: 7).fill()
    }
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach { removeTrackingArea($0) }
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self, userInfo: nil))
    }
    override func mouseEntered(with event: NSEvent) { drin = true }
    override func mouseExited(with event: NSEvent) { drin = false }
    override func mouseUp(with event: NSEvent) {
        guard bounds.contains(convert(event.locationInWindow, from: nil)) else { return }
        let a = aktion
        trackingMenu?.cancelTracking()
        a?()
    }
}

/// Suchfeld ohne schwarzen Editor-Kasten: der Feld-Editor wird schon beim
/// Fokussieren durchsichtig gemacht, nicht erst beim ersten Tastendruck.
final class SuchFeld: NSTextField {
    override func becomeFirstResponder() -> Bool {
        let ok = super.becomeFirstResponder()
        if let ed = currentEditor() as? NSTextView {
            ed.drawsBackground = false
            ed.backgroundColor = .clear
            ed.insertionPointColor = Ton.text
        }
        return ok
    }
}

final class AppSuche: NSObject, NSTextFieldDelegate {
    let feld: NSTextField
    var zeilen: [(item: NSMenuItem, suchtext: String)] = []
    init(_ f: NSTextField) { feld = f; super.init(); f.delegate = self }
    /// Platzhalter am Ende: haelt die Hoehe des Menues konstant. Gemessen:
    /// schrumpfte das Menue beim Filtern auf wenige Zeilen, schloss AppKit es
    /// mitten im Tippen ("saf", "chr").
    var fueller: NSView?
    var fuellItem: NSMenuItem?
    var fensterGetoent = false { didSet { AppZeile.fensterGetoent = fensterGetoent } }
    func zeilenNeuZeichnen() {
        for z in zeilen { z.item.view?.needsDisplay = true }
        feld.superview?.superview?.needsDisplay = true
    }
    var zeilenHoehe: CGFloat = 46
    var sichtbarMax = 0
    func controlTextDidChange(_ obj: Notification) {
        let q = feld.stringValue.folding(options: [.caseInsensitive, .diacriticInsensitive], locale: .current)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        var sichtbar = 0
        for z in zeilen {
            let zeigen = q.isEmpty || z.suchtext.contains(q)
            z.item.isHidden = !zeigen
            if zeigen { sichtbar += 1 }
        }
        if let f = fueller {
            let fehlend = max(0, min(sichtbarMax, zeilen.count) - sichtbar)
            f.setFrameSize(NSSize(width: f.frame.width, height: max(1, CGFloat(fehlend) * zeilenHoehe)))
            f.needsDisplay = true
            if let i = fuellItem { i.menu?.itemChanged(i) }
        }
    }
    /// Enter oeffnet den ersten sichtbaren Treffer (wie Spotlight).
    func control(_ control: NSControl, textView: NSTextView, doCommandBy sel: Selector) -> Bool {
        guard sel == #selector(NSResponder.insertNewline(_:)) else { return false }
        guard let z = zeilen.first(where: { !$0.item.isHidden }), let zeile = z.item.view as? AppZeile else { return true }
        z.item.menu?.cancelTracking()
        zeile.aktion?()
        return true
    }
    /// Der Feld-Editor malt sonst einen schwarzen Kasten hinter den Text.
    func controlTextDidBeginEditing(_ obj: Notification) { feldEditorRichten() }
    func feldEditorRichten() {
        guard let ed = feld.currentEditor() as? NSTextView else { return }
        ed.drawsBackground = false
        ed.backgroundColor = .clear
        ed.insertionPointColor = Ton.text
        ed.textColor = Ton.text
    }
}

/// Kurze Rueckmeldung im Fussband: [Symbol] Programm · Fenstertitel. Hat
/// ihren eigenen Platz im Band und liegt nie ueber dem Inhalt eines Fensters.
final class HinweisZeile: NSView {
    let icon = NSImageView()
    let text = NSTextField(labelWithString: "")
    override init(frame: NSRect) {
        super.init(frame: frame)
        icon.imageScaling = .scaleProportionallyUpOrDown
        text.lineBreakMode = .byTruncatingTail
        text.cell?.truncatesLastVisibleLine = true
        addSubview(icon); addSubview(text)
        alphaValue = 0
        isHidden = true
    }
    required init?(coder: NSCoder) { fatalError() }
    func setzen(bild: NSImage?, name: String, titel: String) {
        icon.image = bild
        icon.isHidden = bild == nil
        let a = NSMutableAttributedString(string: name, attributes: [
            .font: NSFont.systemFont(ofSize: 10.5, weight: .semibold), .foregroundColor: Ton.text])
        if !titel.isEmpty {
            a.append(NSAttributedString(string: "  " + titel, attributes: [
                .font: NSFont.systemFont(ofSize: 10.5, weight: .regular), .foregroundColor: Ton.text2]))
        }
        text.attributedStringValue = a
        needsLayout = true
    }
    var bedarf: CGFloat { (icon.isHidden ? 0 : 19) + text.attributedStringValue.size().width + 6 }
    override func layout() {
        super.layout()
        let x: CGFloat = icon.isHidden ? 0 : 19
        icon.frame = NSRect(x: 0, y: (bounds.height - 15) / 2, width: 15, height: 15)
        text.frame = NSRect(x: x, y: (bounds.height - 14) / 2, width: max(0, bounds.width - x), height: 14)
    }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

/// Wo der Zeiger auf einem Fenster der Buehne steht.
enum Zone: Equatable {
    case ampel(Int)       // 0 schliessen, 1 minimieren, 2 maximieren
    case rand(Int)        // Maske: links 1, rechts 2, oben 4, unten 8
    case inhalt           // alles andere (Kopfband entscheidet Noki per AX)
}

/// Resize-Zeiger fuer eine Kantenmaske (macOS 15+: echte Rahmen-Zeiger).
func randZeiger(_ maske: Int) -> NSCursor {
    let pos: NSCursor.FrameResizePosition
    switch maske {
    case 1: pos = .left
    case 2: pos = .right
    case 4: pos = .top
    case 8: pos = .bottom
    case 5: pos = .topLeft
    case 6: pos = .topRight
    case 9: pos = .bottomLeft
    default: pos = .bottomRight
    }
    return NSCursor.frameResize(position: pos, directions: .all)
}

final class Ansicht: NSView {
    let buehne = CALayer()          // der Schreibtisch (Hintergrund + Fenster)
    let wand = CALayer()
    let leer = CATextLayer()
    let fuss = CALayer()
    let label = NSTextField(labelWithString: "Noki Schreibtisch")
    /// Kurze Rueckmeldung (Kuerzel: welches Fenster jetzt vorn ist). Liegt
    /// ueber der Buehne, damit sie auch kompakt zu sehen ist.
    let hinweis = HinweisZeile(frame: .zero)
    var hinweisItem: DispatchWorkItem?
    var hinweisAn = false
    /// Echte Ampel-Knoepfe je Fenster (relativ zum Fenster, virtuelle Punkte).
    var ampelRahmen: [UInt32: [CGRect]] = [:]
    /// Fenster, die ihre Groesse nicht aendern lassen (z. B. Rechner): keine
    /// Resize-Zonen und keine Resize-Zeiger.
    var festeGroesse: Set<UInt32> = []
    /// Fenster, ueber dessen Ampeln der Zeiger gerade steht (Symbole zeigen).
    var ampelSchwebe: UInt32 = 0
    /// Fenster-Zug, den der Helfer selbst mit dem Zeiger fuehrt (Noki hat
    /// ihn bestaetigt): Startpunkt + Startrahmen, virtuelle Koordinaten.
    var lokalerZug: (wid: UInt32, art: Int, start: CGPoint, rahmen: CGRect)?
    /// Nach dem Loslassen: Umfrage-Rahmen erst wieder uebernehmen, wenn Noki
    /// den endgueltigen Rahmen gemeldet hat.
    var zugSperreBis = Date.distantPast
    var zugSperreWid: UInt32 = 0
    /// Letzte Antwort "ziehbar?" fuer das Kopfband (virtuelle Punkte).
    var kopfAntwort: (wid: UInt32, punkt: CGPoint, ok: Bool)?
    var kopfFrage: (wid: UInt32, punkt: CGPoint, zeit: Date)?
    var letzteMaus = NSPoint.zero
    /// Tastatur geht gerade an ein Feld auf Noki Schreibtisch. Sichtbar nur
    /// als leise getoenter Rand (und gross als Fusszeile) - kein Overlay.
    var tippen = false { didSet { randSetzen(); needsLayout = true } }
    var interaction = false { didSet { randSetzen(); needsLayout = true } }
    // Keine Knoepfe mehr: Groesse macht der Zeiger (hinein = gross, hinaus =
    // kompakt), Sichtbarkeit macht Kuerzel 4. Ein Knopf, der dasselbe noch
    // einmal kann, ist eine zweite Wahrheit - und im Fussband die einzige
    // Stelle, an der ein Klick NICHT den Schreibtisch besucht.
    let navi = NaviKnopf()
    /// Die volle Beschriftung. Im kompakten Fussband ist neben dem Knopf
    /// kein Platz fuer sie - dann steht dort nur "Noki". Abgeschnittener
    /// Text mit Auslassungspunkten sieht nach Fehler aus; eine kuerzere,
    /// vollstaendige Beschriftung nicht.
    var langText = "Noki Schreibtisch" { didSet { needsLayout = true } }
    var fensterEbenen: [UInt32: CALayer] = [:]
    /// Window controls belong to each composed window, never to the
    /// Workspace footer. They deliberately sit above the captured titlebar
    /// so app-specific status/monitor icons cannot masquerade as controls.
    var fensterAmpeln: [UInt32: [CAShapeLayer]] = [:]
    /// macOS zeichnet auf jedes aufgenommene Fenster einen violetten
    /// Aufnahme-Hinweis genau an die Stelle der Ampeln (gemessen, macOS 26).
    /// In der Miniatur deckt ihn eine Flaeche in der Farbe der Titelleiste
    /// dieses Fensters ab - darauf liegen nur Nokis drei Kreise.
    var fensterFlicken: [UInt32: CALayer] = [:]
    var flickenFarbe: [UInt32: CGColor] = [:]
    var orte: [UInt32: CGRect] = [:]
    var abgedeckt = false
    var istGross = false
    var hoverWorkItem: DispatchWorkItem?
    var trackingBereich: NSTrackingArea?
    /// A physical press belongs to this view until its matching release.
    /// Remote work is deliberately queued only from `mouseUp`: starting a
    /// Space bridge from `mouseDown` removed the receiving panel while the
    /// original click was still in flight, so its release could reach the
    /// user's real Desktop.
    var gedruecktesFernziel: (wid: UInt32, punkt: CGPoint, klicks: Int, seq: UInt64, art: Int)?
    var klickSequenz: UInt64 = 0
    /// Target of the running scroll gesture (finger + momentum).
    var scrollGeste: (UInt32, CGPoint)?
    /// Klick oder Ziehen: erst eine Bewegung ueber der Schwelle macht aus
    /// einem Druck eine Zieh-Geste (Auswahl). Darunter bleibt es ein Klick.
    var druckAnsichtPunkt = NSPoint.zero
    var zieht = false
    var letzterZug = Date.distantPast
    /// Rechtsklick (oder Ctrl+Klick): eigenes Ziel, nie ein normaler Klick.
    var rechtsZiel: (wid: UInt32, punkt: CGPoint)?
    var ctrlKlick = false
    /// Native traffic-light controls are intercepted before the generic
    /// click route. In particular green means Noki maximize, never macOS
    /// fullscreen (which would create another physical Space).
    var fensterKontrolle: (wid: UInt32, aktion: String)?
    /// Nokis Markierung fuer nicht editierbaren Seitentext (virtuelle Koordinaten).
    let markEbene = CALayer()
    var markRechtecke: [CGRect] = []
    /// Remote text field that receives Noki's keys (virtual coords).
    var fokusRechteck: CGRect? { didSet { ordnen() } }
    let fokusEbene: CALayer = {
        let l = CALayer()
        l.borderColor = NSColor.controlAccentColor.withAlphaComponent(0.9).cgColor
        l.borderWidth = 2
        l.cornerRadius = 5
        l.zPosition = 5000
        l.isHidden = true
        return l
    }()
    /// Terminal fallback caret. Its rectangle is the real AX insertion
    /// range, never the mouse location. Only Rust enables it for a verified
    /// Terminal typing session when the inactive capture has no solid caret.
    var caretRechteck: CGRect? { didSet { ordnen() } }
    let caretEbene: CALayer = {
        let l = CALayer()
        l.backgroundColor = NSColor(white: 0.96, alpha: 0.94).cgColor
        l.cornerRadius = 0
        l.zPosition = 5001
        l.isHidden = true
        return l
    }()
    /// App-Leiste: registrierte Fenster von Noki Schreibtisch (aus Noki).
    var leiste: [[String: Any]] = []
    var leistenKnoepfe: [LeistenKnopf] = []
    let plusKnopf = LeistenKnopf()
    var katalogWartet = false
    var katalogSuche: AppSuche?

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = NSColor(red: 0.07, green: 0.086, blue: 0.106, alpha: 1).cgColor
        layer?.cornerRadius = 12
        layer?.masksToBounds = true
        layer?.borderWidth = 1
        layer?.borderColor = NSColor(white: 1, alpha: 0.13).cgColor
        buehne.isGeometryFlipped = true
        buehne.masksToBounds = true
        wand.contentsGravity = .resizeAspectFill
        wand.masksToBounds = true
        buehne.addSublayer(wand)
        leer.string = "Noki Schreibtisch ist leer"
        leer.alignmentMode = .center
        leer.foregroundColor = NSColor(white: 0.94, alpha: 0.76).cgColor
        leer.font = NSFont.systemFont(ofSize: 13, weight: .medium)
        leer.fontSize = 13
        leer.contentsScale = NSScreen.main?.backingScaleFactor ?? 2
        buehne.addSublayer(leer)
        layer?.addSublayer(buehne)
        fuss.backgroundColor = NSColor(red: 0.07, green: 0.086, blue: 0.106, alpha: 0.96).cgColor
        layer?.addSublayer(fuss)
        label.font = .systemFont(ofSize: 9.5, weight: .medium)
        label.textColor = NSColor(white: 0.94, alpha: 0.72)
        label.lineBreakMode = .byTruncatingTail
        addSubview(label)
        addSubview(navi)
        addSubview(hinweis)
        markEbene.zPosition = 5000
        buehne.addSublayer(markEbene)
        // "+ Apps" was removed from the Miniatur (2026-09-27): apps are
        // opened natively on the real Noki Desktop. No button, no hitbox.
    }

    func leisteSetzen(_ l: [[String: Any]]) {
        leiste = l
        festeGroesse = Set(l.compactMap { e -> UInt32? in
            ((e["skalierbar"] as? Bool) == false) ? (e["wid"] as? NSNumber)?.uint32Value : nil })
        for e in l {
            guard let wid = (e["wid"] as? NSNumber)?.uint32Value,
                  let a = e["ampeln"] as? [NSNumber], a.count == 12 else { continue }
            let z = a.map { CGFloat($0.doubleValue) }
            ampelRahmen[wid] = (0..<3).map { CGRect(x: z[$0 * 4], y: z[$0 * 4 + 1], width: z[$0 * 4 + 2], height: z[$0 * 4 + 3]) }
        }
        // Nach Programm gruppiert, in Registrierungsreihenfolge.
        var gruppen: [(pfad: String, fenster: [[String: Any]])] = []
        for e in l {
            let p = (e["pfad"] as? String) ?? (e["app"] as? String) ?? ""
            if let i = gruppen.firstIndex(where: { $0.pfad == p }) { gruppen[i].fenster.append(e) }
            else { gruppen.append((p, [e])) }
        }
        // Gleiche Gruppierung: dieselben Knoepfe behalten und nur neu
        // beschriften. Gemessen: das Neuerzeugen nach jedem Klick (die
        // Hervorhebung wechselt) liess den naechsten Klick ins Leere gehen,
        // wenn es zwischen Druecken und Loslassen fiel (8 von 12 kamen an).
        let signatur = gruppen.map { g in g.pfad + ":" + g.fenster.compactMap { ($0["wid"] as? NSNumber)?.stringValue }.joined(separator: ",") }
        let wiederverwenden = signatur == leistenSignatur && leistenKnoepfe.count == gruppen.count
        if !wiederverwenden {
            leistenKnoepfe.forEach { $0.removeFromSuperview() }
            leistenKnoepfe = []
        }
        leistenSignatur = signatur
        for (gi, g) in gruppen.enumerated() {
            let k = wiederverwenden ? leistenKnoepfe[gi] : LeistenKnopf()
            if let ic = g.fenster.first?["icon"] as? String, !ic.isEmpty { k.bild = NSWorkspace.shared.icon(forFile: ic) }
            else if g.pfad.hasSuffix(".app") { k.bild = NSWorkspace.shared.icon(forFile: g.pfad) }
            let name = (g.fenster.first?["app"] as? String) ?? ""
            k.name = name.replacingOccurrences(of: "Google ", with: "")
            k.text = k.name
            k.zahl = g.fenster.count
            k.aktiv = g.fenster.contains { ($0["aktiv"] as? Bool) == true }
            k.hilfe = g.fenster.map { ($0["titel"] as? String) ?? "" }.joined(separator: "\n")
            let fenster = g.fenster
            k.klick = { [weak k] in
                guard let k else { return }
                if fenster.count == 1, let wid = (fenster[0]["wid"] as? NSNumber)?.int64Value {
                    sendeText("APP vorn \(wid)"); return
                }
                // Mehrere Noki-Fenster eines Programms: nie raten - waehlen lassen.
                let m = NSMenu()
                for e in fenster {
                    guard let wid = (e["wid"] as? NSNumber)?.int64Value else { continue }
                    let t = ((e["titel"] as? String) ?? "Fenster") + (((e["minimiert"] as? Bool) == true) ? " (minimiert)" : "")
                    m.addItem(menuePunkt(String(t.prefix(60)), an: (e["aktiv"] as? Bool) == true) { sendeText("APP vorn \(wid)") })
                }
                menueZeigen(m, an: k)
            }
            k.rechts = { [weak k] in
                guard let k else { return }
                let m = NSMenu()
                for e in fenster {
                    guard let wid = (e["wid"] as? NSNumber)?.int64Value else { continue }
                    let t = String(((e["titel"] as? String) ?? "Fenster").prefix(40))
                    m.addItem(menuePunkt("Nach vorn: \(t)") { sendeText("APP vorn \(wid)") })
                    m.addItem(menuePunkt("Minimieren: \(t)") { sendeText("APP min \(wid)") })
                    m.addItem(menuePunkt("Maximieren / Wiederherstellen: \(t)") { sendeText("APP max \(wid)") })
                    m.addItem(menuePunkt("Schließen: \(t)") { sendeText("APP zu \(wid)") })
                }
                menueZeigen(m, an: k)
            }
            if !wiederverwenden {
                addSubview(k)
                leistenKnoepfe.append(k)
            } else {
                k.needsDisplay = true
            }
        }
        needsLayout = true
    }
    var leistenSignatur: [String] = []

    func katalogZeigen(_ apps: [[String: Any]]) {
        guard katalogWartet else { return }
        katalogWartet = false
        AppZeile.fensterGetoent = false
        let m = NSMenu()
        // Dunkle, durchscheinende Menue-Grundflaeche (System-Blur); darauf
        // der Sandton. Helle Schrift bleibt so immer lesbar.
        m.appearance = NSAppearance(named: .darkAqua)
        m.minimumWidth = 344
        let suchFlaeche = SandFlaeche(frame: NSRect(x: 0, y: 0, width: 344, height: 54))
        // Das Feld: eine leicht hellere, halbtransparente Flaeche mit feinem
        // Sandrand; Lupe und Text vertikal mittig. Kein weisser Kasten.
        let kasten = NSView(frame: NSRect(x: 12, y: 12, width: 320, height: 30))
        kasten.wantsLayer = true
        kasten.layer?.backgroundColor = Ton.feld.cgColor
        kasten.layer?.cornerRadius = 8
        kasten.layer?.borderColor = Ton.feldRand.cgColor
        kasten.layer?.borderWidth = 1
        let lupe = NSImageView(frame: NSRect(x: 10, y: 8, width: 14, height: 14))
        lupe.image = NSImage(systemSymbolName: "magnifyingglass", accessibilityDescription: nil)?
            .withSymbolConfiguration(.init(pointSize: 12, weight: .medium))
        lupe.contentTintColor = Ton.text2
        kasten.addSubview(lupe)
        let suchfeld = SuchFeld(frame: NSRect(x: 30, y: 6, width: 282, height: 18))
        suchfeld.font = .systemFont(ofSize: 13.5)
        suchfeld.textColor = Ton.text
        suchfeld.placeholderAttributedString = NSAttributedString(string: "App suchen", attributes: [
            .foregroundColor: Ton.text2, .font: NSFont.systemFont(ofSize: 13.5)])
        suchfeld.focusRingType = .none
        suchfeld.isBordered = false
        suchfeld.isBezeled = false
        suchfeld.drawsBackground = false
        suchfeld.appearance = NSAppearance(named: .darkAqua)
        suchfeld.cell?.usesSingleLineMode = true
        suchfeld.cell?.isScrollable = true
        kasten.addSubview(suchfeld)
        suchFlaeche.addSubview(kasten)
        let suchItem = NSMenuItem(); suchItem.view = suchFlaeche; m.addItem(suchItem)
        let suche = AppSuche(suchfeld)
        for a in apps {
            guard let name = a["name"] as? String, let pfad = a["pfad"] as? String else { continue }
            let item = NSMenuItem()
            let zeile = AppZeile(name: name, bild: NSWorkspace.shared.icon(forFile: (a["icon"] as? String) ?? pfad),
                                 status: (a["status"] as? String) ?? "")
            zeile.trackingMenu = m
            zeile.aktion = { [weak self] in
                self?.hinweisZeigen("\(name) wird in Noki geöffnet …")
                sendeText("APP neu \(pfad)")
            }
            item.view = zeile
            m.addItem(item)
            let aliases = (a["aliases"] as? [String]) ?? []
            let suchtext = ([name] + aliases).joined(separator: " ")
                .folding(options: [.caseInsensitive, .diacriticInsensitive], locale: .current)
            suche.zeilen.append((item, suchtext))
        }
        // So viele Zeilen, wie auf den Bildschirm passen - so hoch bleibt das
        // Menue auch beim Filtern.
        let hoehe = (NSScreen.main?.visibleFrame.height ?? 800) - 120
        suche.sichtbarMax = max(1, Int((hoehe - 54) / 46))
        let fueller = SandFlaeche(frame: NSRect(x: 0, y: 0, width: 344, height: 1))
        fueller.linie = false

        let fuellItem = NSMenuItem(); fuellItem.view = fueller; m.addItem(fuellItem)
        suche.fueller = fueller
        suche.fuellItem = fuellItem
        DispatchQueue.main.async {
            // Ein Sandton fuer das ganze Menue (ueber dem System-Blur).
            func blurs(_ v: NSView) -> [NSVisualEffectView] {
                ((v as? NSVisualEffectView).map { [$0] } ?? []) + v.subviews.flatMap { blurs($0) }
            }
            if let fenster = suchfeld.window, let rahmen = fenster.contentView?.superview ?? fenster.contentView {
                let alle = blurs(rahmen)
                for e in alle {
                    let ton = NSView(frame: e.bounds)
                    ton.wantsLayer = true
                    ton.layer?.backgroundColor = Ton.flaeche.cgColor
                    ton.autoresizingMask = [.width, .height]
                    e.addSubview(ton, positioned: .below, relativeTo: nil)
                }
                if !alle.isEmpty {
                    suche.fensterGetoent = true
                    suche.zeilenNeuZeichnen()
                }
            }
            suchfeld.window?.makeFirstResponder(suchfeld)
            suche.feldEditorRichten()
            // Der Feld-Editor wird erst im naechsten Durchlauf eingesetzt.
            for t in [0.02, 0.08, 0.2] {
                DispatchQueue.main.asyncAfter(deadline: .now() + t) { suche.feldEditorRichten() }
            }
        }
        katalogSuche = suche // retain across NSMenu's nested tracking loop
        menueZeigen(m, an: plusKnopf)
        katalogSuche = nil
    }

    func markierungSetzen(_ r: [CGRect]) {
        markRechtecke = r
        CATransaction.begin(); CATransaction.setDisableActions(true)
        markEbene.sublayers?.forEach { $0.removeFromSuperlayer() }
        for _ in r {
            let l = CALayer()
            l.backgroundColor = NSColor.selectedTextBackgroundColor.withAlphaComponent(0.55).cgColor
            l.cornerRadius = 1.5
            markEbene.addSublayer(l)
        }
        CATransaction.commit()
        ordnen()
    }

    /// Punkt in der Ansicht -> Schreibtisch, an den Rand der Buehne geklemmt
    /// (eine Auswahl darf ueber den Rand hinaus weitergezogen werden).
    func aufSchreibtischGeklemmt(_ p: NSPoint) -> CGPoint {
        let b = buehne.frame
        let q = NSPoint(x: min(max(p.x, b.minX + 0.5), b.maxX - 0.5), y: min(max(p.y, b.minY + 0.5), b.maxY - 0.5))
        return aufSchreibtisch(q) ?? CGPoint(x: schreibtisch.midX, y: schreibtisch.midY)
    }

    func randSetzen() {
        // Derselbe warme Sandton wie der Fussknopf - keine neue Farbe.
        layer?.borderColor = (tippen || interaction)
            ? NSColor(red: 0.89, green: 0.871, blue: 0.831, alpha: 0.62).cgColor
            : NSColor(white: 1, alpha: 0.13).cgColor
        layer?.borderWidth = (tippen || interaction) ? 1.5 : 1
    }

    func hinweisZeigen(_ text: String) { hinweisZeigen(bild: nil, name: text, titel: "") }

    func hinweisZeigen(bild: NSImage?, name: String, titel: String) {
        hinweisItem?.cancel()
        guard !name.isEmpty || !titel.isEmpty else { return }
        hinweis.setzen(bild: bild, name: name, titel: titel)
        hinweisAn = true
        hinweis.isHidden = false
        ordnen()
        NSAnimationContext.runAnimationGroup { c in c.duration = 0.12; hinweis.animator().alphaValue = 1 }
        let item = DispatchWorkItem { [weak self] in
            guard let self else { return }
            NSAnimationContext.runAnimationGroup({ c in c.duration = 0.25; self.hinweis.animator().alphaValue = 0 },
                completionHandler: { self.hinweisAn = false; self.hinweis.isHidden = true; self.ordnen() })
        }
        hinweisItem = item
        DispatchQueue.main.asyncAfter(deadline: .now() + 2.2, execute: item)
    }
    required init?(coder: NSCoder) { fatalError() }
    override var isFlipped: Bool { false }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    /// Der Knopf im Fussband braucht seine EIGENE Trefferflaeche.
    ///
    /// Diese Ueberschreibung gab bisher immer `self` zurueck - geschrieben zu
    /// einer Zeit, als die Ansicht keine bedienbaren Unteransichten hatte.
    /// Seit es den Knopf gibt, verschluckte sie JEDEN Druck auf ihn: die
    /// Ereignisse landeten in `Ansicht.mouseDown`, also in der
    /// Fernbedienung, und "Zum Schreibtisch N" tat nie etwas. Genau das hat
    /// der Nutzer gemeldet.
    override func hitTest(_ punkt: NSPoint) -> NSView? {
        // AppKit liefert `punkt` in Koordinaten der SUPERVIEW. Seit die
        // Ansicht in der festen Huelle sitzt (Versatz != 0), muss hier echt
        // umgerechnet werden - sonst trafen Klicks (z. B. "Schliessen" der
        // Vollansicht) daneben.
        let p = superview.map { convert(punkt, from: $0) } ?? punkt
        guard bounds.contains(p) else { return nil }
        if !navi.isHidden, navi.frame.contains(p) { return navi }
        for k in leistenKnoepfe where !k.isHidden && k.frame.contains(p) { return k }
        return self
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let t = trackingBereich { removeTrackingArea(t) }
        let t = NSTrackingArea(rect: bounds,
                               options: [.mouseEnteredAndExited, .mouseMoved,
                                         .activeAlways, .inVisibleRect],
                               owner: self,
                               userInfo: nil)
        addTrackingArea(t)
        trackingBereich = t
        // Neue Trefferzone: die alte Enter/Exit-Geschichte gilt nicht mehr.
        DispatchQueue.main.async { hoverAbgleichen() }
    }

    /// Nach einem Schreibtischwechsel ist der Hover GESPERRT.
    ///
    /// Sonst genuegte es, dass der Zeiger zufaellig dort stand, wo die
    /// grosse Miniatur spaeter liegen wuerde - sie klappte beim Ankommen
    /// sofort auf. Ein Schreibtischwechsel ist aber keine Zeigerbewegung.
    /// Erst eine echte Bewegung ueber der kompakten Flaeche macht ihn
    /// wieder scharf; blosses Dastehen reicht nicht.
    var hoverGesperrt = false

    override func mouseMoved(with event: NSEvent) {
        hoverGesperrt = false
        hoverAbgleichen()
        letzteMaus = convert(event.locationInWindow, from: nil)
        zeigerAbgleichen()
    }

    override func mouseEntered(with event: NSEvent) {
        guard !abgedeckt, sichtbar else { return }
        hoverGesperrt = false
        hoverWorkItem?.cancel()
        hoverWorkItem = nil
        hoverAbgleichen()
    }

    override func mouseExited(with event: NSEvent) {
        if gedruecktesFernziel == nil && lokalerZug == nil { ampelHover(0); NSCursor.arrow.set() }
        hoverGesperrt = false
        hoverWorkItem?.cancel()
        hoverWorkItem = nil
        hoverAbgleichen()
    }

    override func layout() {
        super.layout()
        ordnen()
    }

    /// Alles aus den aktuellen Grenzen - dieselbe Rechnung fuer jede Groesse.
    func ordnen() {
        CATransaction.begin(); CATransaction.setDisableActions(true)
        let f = abgedeckt ? 0 : FUSS
        let w = bounds.width, h = bounds.height
        buehne.frame = CGRect(x: 0, y: f, width: w, height: max(0, h - f))
        wand.frame = buehne.bounds
        leer.frame = CGRect(x: 16, y: max(0, (buehne.bounds.height - 18) / 2),
                            width: max(0, buehne.bounds.width - 32), height: 18)
        leer.isHidden = !fensterEbenen.isEmpty
        fuss.frame = CGRect(x: 0, y: 0, width: w, height: f)
        fuss.isHidden = abgedeckt
        label.isHidden = abgedeckt
        navi.isHidden = abgedeckt
        // EIN linksbuendiges Paar: Beschriftung, daneben der Knopf. Am
        // rechten Rand wirkte er wie ein zweites, fremdes Bedienelement -
        // er gehoert aber zur selben Aussage ("das ist Nokis Schreibtisch,
        // hier geht es hin"). Der Knopf hat trotzdem seine EIGENE Flaeche:
        // nur hier wird navigiert, alles darueber ist Inhalt.
        // Der Knopf bekommt zuerst, was er braucht - er traegt die Nummer
        // und muss lesbar bleiben. Was uebrig ist, gehoert der Beschriftung;
        // sie darf kuerzen, der Knopf nicht.
        let nb = min(navi.breite, max(0, w - 24))
        let mass: [NSAttributedString.Key: Any] = [.font: label.font ?? NSFont.systemFont(ofSize: 9.5)]
        let vollBreite = (langText as NSString).size(withAttributes: mass).width + 2
        let platz = w - nb - 24
        // Kompakt ist das Fussband schmal: dort steht nur "Noki", daneben der
        // Knopf mit der Nummer. Gross passt die volle Beschriftung. Gemessen
        // reichte die reine Breitenrechnung nicht - die Kompaktbreite steht
        // beim ersten Ordnen noch nicht fest.
        // Im kompakten Fussband steht NUR der Knopf. Er sagt ohnehin alles
        // (er traegt die Nummer), und eine zweite, halb abgeschnittene
        // Beschriftung daneben sieht nach Fehler aus. Gross ist Platz fuer
        // beides - dann bilden sie ein linksbuendiges Paar.
        // Mit Sicherheitsabstand: die gemessene Breite und die wirklich
        // gezeichnete gehen bei dieser kleinen Schrift um ein paar Punkte
        // auseinander, und ein abgeschnittenes "Noki Schreibtis..." sieht
        // nach Fehler aus.
        let text = tippen ? "Tastatur geht an Noki Schreibtisch · Esc beendet" : langText
        let textBreite = (text as NSString).size(withAttributes: mass).width + 2
        let passt = (istGross || vollAn) && textBreite + 16 <= platz
        if label.stringValue != text { label.stringValue = text }
        label.isHidden = abgedeckt || !passt
        _ = vollBreite
        let lb = passt ? textBreite + 6 : 0
        label.frame = NSRect(x: 8, y: 6, width: lb, height: 13)
        navi.frame = NSRect(x: 8 + lb + (lb > 0 ? 8 : 0), y: 3, width: nb, height: FUSS - 6)
        // App-Leiste rechts im Fussband - nur gross/voll, nie im kompakten Band.
        let leisteSichtbar = (istGross || vollAn) && !abgedeckt
        var rx = w - 8
        let hk = FUSS - 6
        for k in leistenKnoepfe.reversed() {
            // Im grossen (nicht vollen) Band nur Symbole, damit alles passt.
            if k.bild != nil { k.text = vollAn ? k.name : "" }
            let kb = k.breite
            let passt = leisteSichtbar && rx - kb > navi.frame.maxX + 8
            k.isHidden = !passt
            if passt { rx -= kb; k.frame = NSRect(x: rx, y: 3, width: kb, height: hk); rx -= 4 }
        }
        // Rueckmeldung: im Fussband rechts neben dem Knopf, links von der
        // App-Leiste. Reicht der Platz nicht, tritt sie an die Stelle von
        // Beschriftung und Knopf - nie ueber den Inhalt eines Fensters.
        if hinweisAn && !abgedeckt {
            let rechts = leistenKnoepfe.filter { !$0.isHidden }.map { $0.frame.minX }.min() ?? (w - 8)
            var x0 = navi.frame.maxX + 12
            if rechts - 8 - x0 < min(150, hinweis.bedarf) {
                x0 = 10
                label.isHidden = true
                navi.isHidden = true
            }
            hinweis.frame = NSRect(x: x0, y: 0, width: max(0, min(hinweis.bedarf, rechts - 8 - x0)), height: f)
        }
        let sx = buehne.bounds.width / max(1, schreibtisch.width)
        let sy = buehne.bounds.height / max(1, schreibtisch.height)
        // Snap window layers to the physical pixel grid: a 1:1 capture drawn
        // at a fractional origin is resampled again and loses its sharpness.
        let px = window?.backingScaleFactor ?? NSScreen.main?.backingScaleFactor ?? 2
        let raster: (CGFloat) -> CGFloat = { ($0 * px).rounded() / px }
        for (id, l) in fensterEbenen {
            guard let o = orte[id] else { continue }
            let x0 = raster((o.minX - schreibtisch.minX) * sx), y0 = raster((o.minY - schreibtisch.minY) * sy)
            l.frame = CGRect(x: x0, y: y0,
                             width: max(2, raster((o.minX - schreibtisch.minX + o.width) * sx) - x0),
                             height: max(2, raster((o.minY - schreibtisch.minY + o.height) * sy) - y0))
            let registriert = leiste.contains {
                (($0["wid"] as? NSNumber)?.uint32Value ?? 0) == id
            }
            let symbole = ampelSchwebe == id
            if let fl = fensterFlicken[id] {
                let r = (0..<3).map { ampelVirtuell(id, $0) }.reduce(CGRect.null) { $0.union($1) }
                    .insetBy(dx: -5, dy: -4)
                let k = CGRect(x: r.minX * sx, y: r.minY * sy, width: r.width * sx, height: r.height * sy)
                fl.frame = k
                fl.cornerRadius = k.height / 2
                fl.backgroundColor = flickenFarbe[id]
                fl.isHidden = !registriert || flickenFarbe[id] == nil
            }
            for (i, a) in (fensterAmpeln[id] ?? []).enumerated() {
                // Genau ueber dem ECHTEN Knopf des Programms (von Noki per AX
                // gemessen) und etwas groesser - so bleibt vom grauen Knopf
                // eines inaktiven Fensters nichts sichtbar.
                let k = ampelKreis(id, i, sx: sx, sy: sy)
                a.frame = l.bounds
                a.isHidden = !registriert
                a.path = CGPath(ellipseIn: k, transform: nil)
                if let g = a.sublayers?.first as? CAShapeLayer {
                    g.frame = l.bounds
                    g.isHidden = !symbole || !registriert
                    g.path = ampelSymbol(i, in: k)
                    g.lineWidth = max(0.6, k.width * 0.09)
                }
            }
        }
        if fokusEbene.superlayer == nil { buehne.addSublayer(fokusEbene) }
        if let o = fokusRechteck {
            fokusEbene.frame = CGRect(x: (o.minX - schreibtisch.minX) * sx - 3, y: (o.minY - schreibtisch.minY) * sy - 3,
                                      width: max(4, o.width * sx) + 6, height: max(4, o.height * sy) + 6)
            fokusEbene.isHidden = false
        } else { fokusEbene.isHidden = true }
        if caretEbene.superlayer == nil { buehne.addSublayer(caretEbene) }
        if let o = caretRechteck {
            caretEbene.frame = CGRect(x: (o.minX - schreibtisch.minX) * sx,
                                      y: (o.minY - schreibtisch.minY) * sy,
                                      width: max(2, o.width * sx),
                                      height: max(4, o.height * sy))
            caretEbene.isHidden = false
            if caretEbene.animation(forKey: "noki-caret-blink") == nil {
                let a = CABasicAnimation(keyPath: "opacity")
                a.fromValue = 1.0; a.toValue = 0.16
                a.duration = 0.52; a.autoreverses = true
                a.repeatCount = .infinity
                a.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
                caretEbene.add(a, forKey: "noki-caret-blink")
            }
        } else {
            caretEbene.isHidden = true
            caretEbene.removeAnimation(forKey: "noki-caret-blink")
        }
        markEbene.frame = buehne.bounds
        for (l, o) in zip(markEbene.sublayers ?? [], markRechtecke) {
            l.frame = CGRect(x: (o.minX - schreibtisch.minX) * sx, y: (o.minY - schreibtisch.minY) * sy,
                             width: max(1, o.width * sx), height: max(1, o.height * sy))
        }
        CATransaction.commit()
    }

    /// Die echten Ampel-Rahmen (virtuell, relativ zum Fenster) - oder die
    /// Standardlage eines macOS-Fensters, solange Noki sie nicht gemessen hat.
    func ampelVirtuell(_ id: UInt32, _ i: Int) -> CGRect {
        if let r = ampelRahmen[id], r.count == 3 { return r[i] }
        return CGRect(x: 12 + CGFloat(i) * 23, y: 12, width: 16, height: 16)
    }

    /// Kreis in Ebenen-Koordinaten (oben-links, wie die Buehne).
    ///
    /// Sichtbare Groesse wie ein echtes macOS-Fenster (12 pt) mal
    /// Ansichtsmassstab, begrenzt auf 7...12 pt: in der Vollansicht fast
    /// normal, in der Miniatur kleiner, aber erkennbar. Die Mitte kommt aus
    /// dem festen Abstand des Knopfes zur Fensterecke (einmal gemessen, aendert
    /// sich weder beim Ziehen noch beim Resize) - die Kreise sind Unterebenen
    /// der Fensterebene und bewegen sich mit ihr im selben Bild.
    func ampelKreis(_ id: UInt32, _ i: Int, sx: CGFloat, sy: CGFloat) -> CGRect {
        // Massstab: die LARGE-Miniatur (Nutzer: "genau richtig") zeigt 7 pt
        // bei 0,48 -> ~14,5 virtuelle pt. Dieselbe Proportion fuer alle
        // Ansichten: COMPACT kleiner, FULL etwas groesser - nur unten/oben
        // begrenzt, damit er erkennbar bzw. macOS-typisch bleibt.
        let r = ampelVirtuell(id, i)
        let d = min(14, max(3.5, 14.5 * min(sx, sy)))
        return CGRect(x: r.midX * sx - d / 2, y: r.midY * sy - d / 2, width: d, height: d)
    }

    /// Unsichtbare Trefferflaeche eines Knopfes: groesser als der Kreis (in
    /// Bildschirmpunkten bequem), aber nie ueber die halbe Lueke zum Nachbarn.
    func ampelTreffer(_ id: UInt32, _ i: Int, sx: CGFloat, sy: CGFloat) -> CGRect {
        let k = ampelKreis(id, i, sx: sx, sy: sy)
        let abstand = (ampelVirtuell(id, 1).midX - ampelVirtuell(id, 0).midX) * sx
        let r = min(max(k.width / 2 + 4, 6), max(k.width / 2 + 1, abstand / 2))
        return CGRect(x: k.midX - r, y: k.midY - r, width: 2 * r, height: 2 * r)
    }

    /// x, −, und das Vollbild-Zeichen (zwei Dreiecke) - wie macOS.
    func ampelSymbol(_ i: Int, in k: CGRect) -> CGPath {
        let p = CGMutablePath()
        let m = CGPoint(x: k.midX, y: k.midY), r = k.width * 0.25
        switch i {
        case 0:
            p.move(to: CGPoint(x: m.x - r, y: m.y - r)); p.addLine(to: CGPoint(x: m.x + r, y: m.y + r))
            p.move(to: CGPoint(x: m.x - r, y: m.y + r)); p.addLine(to: CGPoint(x: m.x + r, y: m.y - r))
        case 1:
            p.move(to: CGPoint(x: m.x - r * 1.15, y: m.y)); p.addLine(to: CGPoint(x: m.x + r * 1.15, y: m.y))
        default:
            // Zwei kleine Dreiecke in den Ecken, mit deutlicher Diagonal-Luecke.
            let q = r * 0.95
            p.move(to: CGPoint(x: m.x - q, y: m.y - q)); p.addLine(to: CGPoint(x: m.x + q * 0.25, y: m.y - q))
            p.addLine(to: CGPoint(x: m.x - q, y: m.y + q * 0.25)); p.closeSubpath()
            p.move(to: CGPoint(x: m.x + q, y: m.y + q)); p.addLine(to: CGPoint(x: m.x - q * 0.25, y: m.y + q))
            p.addLine(to: CGPoint(x: m.x + q, y: m.y - q * 0.25)); p.closeSubpath()
        }
        return p
    }

    /// Titelleisten-Farbe rechts neben den Ampeln aus dem neuesten Bild.
    func flickenFarbeMessen(_ id: UInt32, _ px: CVPixelBuffer) {
        guard let o = orte[id], o.width > 1, o.height > 1,
              CVPixelBufferGetPixelFormatType(px) == kCVPixelFormatType_32BGRA else { return }
        let bw = CVPixelBufferGetWidth(px), bh = CVPixelBufferGetHeight(px)
        // UEBER der Knopfgruppe (der violette Hinweis beginnt erst auf Hoehe
        // der Knoepfe): dort ist reine Titelleiste - rechts daneben lag in
        // Chrome der dunkle "Tab-Suche"-Knopf und faerbte die Flaeche falsch.
        let links = ampelVirtuell(id, 0), rechts = ampelVirtuell(id, 2)
        let vx = (links.minX + rechts.maxX) / 2, vy = max(2, links.minY - 6)
        let x = Int(vx / o.width * CGFloat(bw)), y = Int(vy / o.height * CGFloat(bh))
        guard x >= 8, y >= 1, x < bw - 8, y < bh - 1 else { return }
        CVPixelBufferLockBaseAddress(px, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(px, .readOnly) }
        guard let basis = CVPixelBufferGetBaseAddress(px) else { return }
        let zeile = CVPixelBufferGetBytesPerRow(px)
        let p = basis.assumingMemoryBound(to: UInt8.self)
        var b = 0, g = 0, r = 0
        var n = 0
        for dy in -1...1 { for dx in stride(from: -8, through: 8, by: 2) {
            let i = (y + dy) * zeile + (x + dx) * 4
            b += Int(p[i]); g += Int(p[i + 1]); r += Int(p[i + 2]); n += 1
        } }
        let q = CGFloat(n) * 255
        let neu = CGColor(srgbRed: CGFloat(r) / q, green: CGFloat(g) / q, blue: CGFloat(b) / q, alpha: 1)
        if flickenFarbe[id] == nil || flickenFarbe[id] != neu {
            flickenFarbe[id] = neu
            CATransaction.begin(); CATransaction.setDisableActions(true)
            fensterFlicken[id]?.backgroundColor = neu
            fensterFlicken[id]?.isHidden = !registriert(id)
            CATransaction.commit()
        }
    }

    func ebene(_ id: UInt32) -> CALayer {
        if let l = fensterEbenen[id] { return l }
        let l = CALayer()
        l.contentsGravity = .resize
        // Verkleinern mit Mip-Stufen statt Punktabtastung: Schrift bleibt
        // in der Miniatur glatt statt pixelig.
        l.minificationFilter = .trilinear
        l.magnificationFilter = .linear
        // Kein Bild -> gar nichts zeichnen. Nie eine schwarze Attrappe.
        l.isHidden = true
        // Dieselben Farben wie die Ampel in Noki Einstellungen. Kein Rand,
        // keine Linie - nur drei saubere Kreise. Die Zeichen (x − ⤢) sind
        // eine eigene Ebene und erscheinen nur beim Darueberfahren.
        let farben = [
            NSColor(red: 1.0, green: 0.373, blue: 0.341, alpha: 1),    // #ff5f57
            NSColor(red: 0.996, green: 0.737, blue: 0.180, alpha: 1),  // #febc2e
            NSColor(red: 0.157, green: 0.784, blue: 0.251, alpha: 1),  // #28c840
        ]
        let flicken = CALayer()
        flicken.zPosition = 19
        flicken.isHidden = true
        l.addSublayer(flicken)
        fensterFlicken[id] = flicken
        fensterAmpeln[id] = farben.enumerated().map { (i, farbe) in
            let a = CAShapeLayer()
            a.fillColor = farbe.cgColor
            a.strokeColor = nil
            a.lineWidth = 0
            a.zPosition = 20
            let g = CAShapeLayer()
            g.strokeColor = NSColor(white: 0, alpha: 0.58).cgColor
            g.fillColor = i == 2 ? NSColor(white: 0, alpha: 0.58).cgColor : nil
            g.lineCap = .round
            g.isHidden = true
            a.addSublayer(g)
            l.addSublayer(a)
            return a
        }
        buehne.addSublayer(l)
        fensterEbenen[id] = l
        leer.isHidden = true
        ordnen()
        return l
    }

    func entfernen(_ id: UInt32) {
        fensterEbenen[id]?.removeFromSuperlayer()
        fensterEbenen[id] = nil; fensterAmpeln[id] = nil; orte[id] = nil; ampelRahmen[id] = nil
        fensterFlicken[id] = nil; flickenFarbe[id] = nil
        leer.isHidden = !fensterEbenen.isEmpty
    }

    /// Ein Punkt IN der Miniatur -> Punkt auf Nokis echtem Schreibtisch.
    ///
    /// Keine geschaetzten Abstaende: gerechnet wird mit genau der Abbildung,
    /// mit der `ordnen()` die Ebenen setzt - Buehnenflaeche zu
    /// Schreibtischflaeche. Damit trifft der Klick das, was der Nutzer SIEHT,
    /// in jeder Groesse (kompakt wie gross).
    func aufSchreibtisch(_ p: NSPoint) -> CGPoint? {
        let b = buehne.frame
        guard b.width > 1, b.height > 1, b.contains(p) else { return nil }
        let u = (p.x - b.minX) / b.width
        // Der Bildschirm zaehlt von OBEN, die Ansicht von unten.
        let v = 1 - (p.y - b.minY) / b.height
        return CGPoint(x: schreibtisch.minX + u * schreibtisch.width,
                       y: schreibtisch.minY + v * schreibtisch.height)
    }

    /// Das OBERSTE Fenster an dieser Stelle - dieselbe Reihenfolge, in der
    /// gezeichnet wird. `stapelNachziehen()` haengt die Ebenen von unten nach
    /// oben ein, die letzte liegt also vorn. Damit stimmen Augenschein und
    /// Ziel ueberein; eine getrennte Liste wuerde frueher oder spaeter
    /// auseinanderlaufen.
    func fensterAn(_ p: NSPoint) -> UInt32? {
        let ebenen = ebenenVonVorn()
        // `buehne` is geometry-flipped (window frames use a top-left
        // origin), while AppKit mouse points use a bottom-left origin.
        // Rendering and hit-testing must compare in the same coordinates.
        let bp = CGPoint(x: p.x - buehne.frame.minX,
                         y: buehne.bounds.height - (p.y - buehne.frame.minY))
        for l in ebenen {
            guard !l.isHidden, l.contents != nil else { continue }
            guard let id = fensterEbenen.first(where: { $0.value === l })?.key else { continue }
            if l.frame.contains(bp) {
                return id
            }
        }
        return nil
    }

    /// Fensterebenen von VORN nach hinten - so, wie CoreAnimation sie
    /// zeichnet (zPosition, bei Gleichstand die Reihenfolge). Die Reihenfolge
    /// der Unterebenen allein stimmte nicht immer mit dem Bild ueberein:
    /// ein Druck auf das vorn gezeigte Chrome traf das dahinterliegende Mail.
    func ebenenVonVorn() -> [CALayer] {
        let alle = buehne.sublayers ?? []
        return alle.enumerated().sorted {
            $0.element.zPosition != $1.element.zPosition ? $0.element.zPosition > $1.element.zPosition : $0.offset > $1.offset
        }.map { $0.element }
    }

    func registriert(_ id: UInt32) -> Bool {
        leiste.contains { (($0["wid"] as? NSNumber)?.uint32Value ?? 0) == id }
    }

    /// Welches Fenster und welche Zone liegt unter dem Zeiger - in genau
    /// dieser Rangfolge: Ampeln, Ecken, Kanten, dann Inhalt (das Kopfband
    /// ist Inhalt, bis Noki per AX "ziehbar" sagt). Kanten greifen ein paar
    /// Punkte INNEN und AUSSEN, nie tief im Webinhalt.
    func zoneAn(_ p: NSPoint) -> (wid: UInt32, zone: Zone)? {
        let ebenen = ebenenVonVorn()
        let bp = CGPoint(x: p.x - buehne.frame.minX,
                         y: buehne.bounds.height - (p.y - buehne.frame.minY))
        let sx = buehne.bounds.width / max(1, schreibtisch.width)
        let sy = buehne.bounds.height / max(1, schreibtisch.height)
        let innen: CGFloat = vollAn ? 5 : 4, aussen: CGFloat = vollAn ? 5 : 4, ecke: CGFloat = vollAn ? 14 : 11
        for l in ebenen {
            guard !l.isHidden, l.contents != nil,
                  let id = fensterEbenen.first(where: { $0.value === l })?.key else { continue }
            let f = l.frame
            let reg = registriert(id)
            if !(reg ? f.insetBy(dx: -aussen, dy: -aussen) : f).contains(bp) { continue }
            guard reg else { return (id, .inhalt) }
            for i in 0..<3 where ampelTreffer(id, i, sx: sx, sy: sy)
                .offsetBy(dx: f.minX, dy: f.minY).contains(bp) {
                return (id, .ampel(i))
            }
            if festeGroesse.contains(id) {
                if f.contains(bp) { return (id, .inhalt) }
                continue
            }
            let dl = bp.x - f.minX, dr = f.maxX - bp.x, dt = bp.y - f.minY, db = f.maxY - bp.y
            var m = 0
            if dl <= innen { m |= 1 }
            if dr <= innen { m |= 2 }
            if dt <= innen { m |= 4 }
            if db <= innen { m |= 8 }
            // Ecken: entlang einer Kante ein Stueck weiter als die Kante selbst.
            if m & 3 != 0 { if dt <= ecke { m |= 4 }; if db <= ecke { m |= 8 } }
            if m & 12 != 0 { if dl <= ecke { m |= 1 }; if dr <= ecke { m |= 2 } }
            if m & 3 == 3 { m &= ~(dl < dr ? 2 : 1) }
            if m & 12 == 12 { m &= ~(dt < db ? 8 : 4) }
            if m != 0 { return (id, .rand(m)) }
            if f.contains(bp) { return (id, .inhalt) }
        }
        return nil
    }

    var schwebAlt: (wid: UInt32, punkt: CGPoint, zeit: Date)?

    /// Der Mauszeiger zeigt, was ein Druck hier taete. Nur das Aussehen -
    /// der Zeiger selbst wird nie bewegt.
    var vorwaermWid: UInt32 = 0
    var vorwaermZeit = Date.distantPast
    func zeigerAbgleichen() {
        let p = letzteMaus
        if let z = lokalerZug {
            (z.art == 16 ? NSCursor.closedHand : randZeiger(z.art)).set(); return
        }
        if let g = gedruecktesFernziel, g.art != 0 { randZeiger(g.art).set(); return }
        guard !abgedeckt, !menueOffen, bounds.contains(p), let z = zoneAn(p) else {
            ampelHover(0); NSCursor.arrow.set(); return
        }
        switch z.zone {
        case .ampel:
            ampelHover(z.wid); NSCursor.arrow.set()
        case .rand(let m):
            ampelHover(0); randZeiger(m).set()
        case .inhalt:
            ampelHover(0)
            // Pre-warm the stream of the window under the pointer: a scroll
            // may start any moment, and SCK's frame-rate reconfiguration
            // at gesture start was measured to stall the source up to
            // 714 ms (Miniatur frozen, then a jump). Rate change now happens
            // on hover, before the finger moves. Only this one window.
            if vorwaermWid != z.wid || Date().timeIntervalSince(vorwaermZeit) > 1.0 {
                vorwaermWid = z.wid; vorwaermZeit = Date()
                let w = z.wid
                Task { await aufnahme.vorwaermen(w) }
            }
            // Hover over window content (throttled): Noki forwards it only to
            // pages that react to it (Noki Browser: control bars, sliders).
            if registriert(z.wid), gedruecktesFernziel == nil, let v = aufSchreibtisch(p) {
                if schwebAlt == nil || schwebAlt!.wid != z.wid
                    || (hypot(schwebAlt!.punkt.x - v.x, schwebAlt!.punkt.y - v.y) >= 3
                        && Date().timeIntervalSince(schwebAlt!.zeit) > 0.04) {
                    schwebAlt = (z.wid, v, Date())
                    sendeText("ZEIGER bewege \(z.wid) \(Int(v.x)) \(Int(v.y))")
                }
            }
            guard registriert(z.wid), let v = aufSchreibtisch(p), let o = orte[z.wid],
                  v.y - o.minY <= 100 else { NSCursor.arrow.set(); return }
            // Kopfband? Noki beantwortet das per AX; bis dahin gilt die
            // letzte Antwort in der Naehe.
            if let a = kopfAntwort, a.wid == z.wid, hypot(a.punkt.x - v.x, a.punkt.y - v.y) < 10 {
                (a.ok ? NSCursor.openHand : NSCursor.arrow).set()
            } else {
                NSCursor.arrow.set()
            }
            if gedruecktesFernziel == nil {
                let alt = kopfFrage
                if alt == nil || alt!.wid != z.wid || hypot(alt!.punkt.x - v.x, alt!.punkt.y - v.y) >= 3
                    && Date().timeIntervalSince(alt!.zeit) > 0.04 {
                    kopfFrage = (z.wid, v, Date())
                    sendeText("ZEIGER schwebe \(z.wid) \(Int(v.x)) \(Int(v.y))")
                }
            }
        }
    }

    func ampelHover(_ id: UInt32) {
        guard ampelSchwebe != id else { return }
        ampelSchwebe = id
        ordnen()
    }

    /// EIN KLICK IN DIE MINIATUR IST KEINE REISE MEHR.
    ///
    /// Die Schreibtischflaeche ist Inhalt, den man bedient: der Klick geht an
    /// das echte Fenster auf Nokis Schreibtisch, der Nutzer bleibt stehen.
    /// Navigation hat ihre eigene Stelle - den Knopf im Fussband.
    override func mouseDown(with e: NSEvent) {
        if fernTapHatte(e.timestamp) { return }
        interaction = true
        sendeText("INTERACTION an")
        // Ctrl+Klick ist auf dem Mac ein Rechtsklick.
        if e.modifierFlags.contains(.control) {
            ctrlKlick = true
            rechtsBeginn(convert(e.locationInWindow, from: nil))
            return
        }
        ctrlKlick = false
        druckBeginn(convert(e.locationInWindow, from: nil), klicks: e.clickCount)
    }

    override func mouseUp(with e: NSEvent) {
        if fernTapHatte(e.timestamp) { return }
        if ctrlKlick { ctrlKlick = false; rechtsEnde(); return }
        if let k = fensterKontrolle {
            fensterKontrolle = nil
            sendeText("APP \(k.aktion) \(k.wid)")
            return
        }
        druckEnde(convert(e.locationInWindow, from: nil))
    }

    override func mouseDragged(with e: NSEvent) {
        guard let g = gedruecktesFernziel, !abgedeckt, !ctrlKlick else { return }
        let p = convert(e.locationInWindow, from: nil)
        if !zieht {
            guard hypot(p.x - druckAnsichtPunkt.x, p.y - druckAnsichtPunkt.y) > 4 else { return }
            zieht = true
            sendeText("ZEIGER ziehen_an \(g.wid) \(Int(g.punkt.x)) \(Int(g.punkt.y)) \(g.art)")
        }
        // Ein bestaetigter Fenster-Zug folgt dem Zeiger in JEDEM Ereignis
        // (Startrahmen + Versatz, nie "aktuelle Lage + letzter Schritt").
        // Der Zeiger darf die Miniatur dabei verlassen: AppKit liefert die
        // Zieh-Ereignisse weiter an diese Ansicht, bis die Taste losgeht.
        let q = aufSchreibtischGeklemmt(p)
        if let z = lokalerZug, z.wid == g.wid {
            orte[z.wid] = zugRahmen(z, q)
            ordnen()
        }
        zeigerAbgleichen()
        guard Date().timeIntervalSince(letzterZug) > 0.014 else { return }
        letzterZug = Date()
        sendeText("ZEIGER ziehen \(g.wid) \(Int(q.x)) \(Int(q.y)) \(g.art)")
    }

    /// Dieselbe Rechnung wie Nokis `fensterzug_rahmen`: Lage oder Kanten,
    /// innerhalb von Noki Schreibtisch, mit Mindestgroesse.
    func zugRahmen(_ z: (wid: UInt32, art: Int, start: CGPoint, rahmen: CGRect), _ q: CGPoint) -> CGRect {
        let dx = q.x - z.start.x, dy = q.y - z.start.y
        let b = schreibtisch
        var r = z.rahmen
        if z.art == 16 {
            r.origin.x = min(max(z.rahmen.minX + dx, b.minX), max(b.minX, b.maxX - z.rahmen.width))
            r.origin.y = min(max(z.rahmen.minY + dy, b.minY), max(b.minY, b.maxY - 28))
            return r
        }
        let minW: CGFloat = 240, minH: CGFloat = 160
        if z.art & 1 != 0 {
            let l = min(max(z.rahmen.minX + dx, b.minX), max(b.minX, z.rahmen.maxX - minW))
            r.origin.x = l; r.size.width = z.rahmen.maxX - l
        }
        if z.art & 2 != 0 { r.size.width = min(max(z.rahmen.width + dx, minW), max(minW, b.maxX - z.rahmen.minX)) }
        if z.art & 4 != 0 {
            let t = min(max(z.rahmen.minY + dy, b.minY), max(b.minY, z.rahmen.maxY - minH))
            r.origin.y = t; r.size.height = z.rahmen.maxY - t
        }
        if z.art & 8 != 0 { r.size.height = min(max(z.rahmen.height + dy, minH), max(minH, b.maxY - z.rahmen.minY)) }
        return r
    }

    func rechtsBeginn(_ p: NSPoint) {
        rechtsZiel = nil
        guard !abgedeckt, let z = aufSchreibtisch(p), let wid = fensterAn(p) else { return }
        rechtsZiel = (wid, z)
    }

    func rechtsEnde() {
        if let r = rechtsZiel {
            sendeText("ZEIGER rechts \(r.wid) \(Int(r.punkt.x)) \(Int(r.punkt.y)) 1")
        }
        rechtsZiel = nil
        DispatchQueue.main.async { hoverAbgleichen() }
    }

    override func scrollWheel(with e: NSEvent) {
        if fernTapHatte(e.timestamp) { return }
        rollen(convert(e.locationInWindow, from: nil), e)
    }

    /// Physischer Druck in der Miniatur - aus dem Panel oder dem Mitlese-Tap.
    /// Beide Wege enden hier; genau einer pro Ereignis. REAL_SPACE entfernt
    /// das Panel fuer Fernbedienung nie und wechselt dabei keinen Space.
    func druckBeginn(_ p: NSPoint, klicks: Int) {
        gedruecktesFernziel = nil
        fensterKontrolle = nil
        lokalerZug = nil
        guard !abgedeckt else { return }
        // Leere Schreibtischflaeche: nichts zu bedienen, und ganz sicher
        // keine ungefragte Reise - aber ein offenes Kontextmenue schliesst
        // sich, wie beim Klick daneben auf einem echten Schreibtisch.
        guard let z = zoneAn(p) else { sendeText("LEER klick"); return }
        let wid = z.wid
        sendeText("KLICKART pointerDown region=content hit=MINIATURE_CONTENT_INTERACTION selectedSpaceID=\(nokiSpace) currentPhysicalSpaceID=\(aktiverSpaceID()) targetWindowID=\(wid)")
        // Click, controls, move and resize share the same authoritative
        // front-window route used by app chips and ^+arrow cycling.
        sendeText("APP vorn \(wid)")
        let ziel = aufSchreibtischGeklemmt(p)
        switch z.zone {
        case .ampel(let i):
            fensterKontrolle = (wid, ["zu", "min", "max"][i])
            return
        case .rand(let m):
            // Kante/Ecke: Fenster und Kante stehen ab jetzt fest - bis zum
            // Loslassen wird nicht neu getroffen.
            klickSequenz &+= 1
            druckAnsichtPunkt = p
            zieht = false
            gedruecktesFernziel = (wid, ziel, min(klicks, 2), klickSequenz, m)
            if let r = orte[wid] { lokalerZug = (wid, m, ziel, r) }
            zeigerAbgleichen()
            return
        case .inhalt:
            // ⌘-drag anywhere in a window moves it - one reliable move
            // affordance for EVERY window, including apps whose title band
            // is full of controls (Finder/Mail toolbars), without any Noki
            // overlay covering app controls.
            if NSEvent.modifierFlags.contains(.command), registriert(wid) {
                klickSequenz &+= 1
                druckAnsichtPunkt = p
                zieht = false
                gedruecktesFernziel = (wid, ziel, min(klicks, 2), klickSequenz, 16)
                if let r = orte[wid] { lokalerZug = (wid, 16, ziel, r) }
                zeigerAbgleichen()
                return
            }
        }
        guard aufSchreibtisch(p) != nil else { return }
        // Consume now; do not begin asynchronous remote work until the
        // matching physical mouseUp has also been consumed by this view.
        klickSequenz &+= 1
        druckAnsichtPunkt = p
        zieht = false
        gedruecktesFernziel = (wid, ziel, min(klicks, 2), klickSequenz, 0)
        Task { await aufnahme.erwarteAenderung(wid) }
        sendeText("INPUT click seq=\(klickSequenz) down wid=\(wid)")
    }

    func druckEnde(_ p: NSPoint) {
        if let k = fensterKontrolle {
            fensterKontrolle = nil
            sendeText("APP \(k.aktion) \(k.wid)")
            return
        }
        let gespeichert = gedruecktesFernziel
        gedruecktesFernziel = nil
        if let z = lokalerZug {
            // Bis Noki den WIRKLICHEN Rahmen meldet, bleibt die Ebene, wo der
            // Zeiger sie hingefuehrt hat - kein Zurueckspringen.
            if zieht { orte[z.wid] = zugRahmen(z, aufSchreibtischGeklemmt(p)); ordnen() }
            zugSperreWid = z.wid
            zugSperreBis = Date().addingTimeInterval(1.5)
            lokalerZug = nil
        }
        if let g = gespeichert, zieht {
            zieht = false
            let q = aufSchreibtischGeklemmt(p)
            sendeText("ZEIGER ziehen_aus \(g.wid) \(Int(q.x)) \(Int(q.y)) \(g.art)")
        } else if let g = gespeichert, !abgedeckt {
            sendeText("INPUT click seq=\(g.seq) up wid=\(g.wid)")
            if aufSchreibtisch(p) != nil, fensterAn(p) == g.wid {
                sendeText("ZEIGER klick \(g.wid) \(Int(g.punkt.x)) \(Int(g.punkt.y)) \(g.klicks) \(g.seq)")
            }
        }
        // Ein echter Druck darf die Groesse halten, aber nur bis zu seinem
        // Ende. Danach entscheidet sofort wieder die reale globale Lage.
        DispatchQueue.main.async { hoverAbgleichen(); self.letzteMaus = p; self.zeigerAbgleichen() }
    }

    // Unsupported physical buttons are still owned by the miniature. They
    // must never fall through to the current Desktop.
    override func rightMouseDown(with e: NSEvent) { rechtsBeginn(convert(e.locationInWindow, from: nil)) }
    override func rightMouseUp(with e: NSEvent) { rechtsEnde() }
    override func otherMouseDown(with e: NSEvent) { }
    override func otherMouseUp(with e: NSEvent) { }

    func rollen(_ p: NSPoint, _ e: NSEvent) {
        guard !abgedeckt else { return }
        // One gesture = one target. Window and point are resolved when the
        // finger starts (or per packet for a classic mouse wheel) and kept
        // for the rest of the gesture INCLUDING momentum - no per-packet
        // re-resolution, no drift onto a neighbouring window or control.
        // Momentum is scroll too (natural continuation after lifting the
        // finger); Noki coalesces it, so it no longer floods transactions.
        let beginnt = e.phase == .began || (e.phase.isEmpty && e.momentumPhase.isEmpty)
        let ziel: CGPoint, wid: UInt32
        if beginnt || scrollGeste == nil {
            guard let z = aufSchreibtisch(p), let w = fensterAn(p) else { return }
            ziel = z; wid = w
            scrollGeste = (w, z)
        } else {
            (wid, ziel) = scrollGeste!
        }
        if e.momentumPhase == .ended || e.momentumPhase == .cancelled
            || (e.phase == .ended && e.momentumPhase.isEmpty && !e.hasPreciseScrollingDeltas) { scrollGeste = nil }
        // Preserve sub-line trackpad packets.  The previous per-packet
        // `/10` followed by `rounded()` turned most real gestures into an
        // endless stream of 0,0 packets before Rust could coalesce them.
        // Exact float deltas, no quantization here (was: Int truncation of
        // finger points * 0.25 = 1 unit per 4 pt = 25.6 px page steps at slow
        // speed - the "small jumps"). Units stay finger points * 0.25 for
        // precise deltas; Noki quantizes per route (AX/keys need whole
        // steps, the Noki Browser takes the float as is).
        let skala = e.hasPreciseScrollingDeltas ? 0.25 : 1.0
        let fdx = e.scrollingDeltaX * skala, fdy = e.scrollingDeltaY * skala
        // Phase: b began, c changed, e finger up, M momentum begin, m momentum,
        // E momentum end, w classic wheel notch.
        let phase: String
        if !e.hasPreciseScrollingDeltas { phase = "w" }
        else if e.momentumPhase == .began { phase = "M" }
        else if e.momentumPhase == .ended || e.momentumPhase == .cancelled { phase = "E" }
        else if !e.momentumPhase.isEmpty { phase = "m" }
        else if e.phase == .began || e.phase == .mayBegin { phase = "b" }
        else if e.phase == .ended || e.phase == .cancelled { phase = "e" }
        else { phase = "c" }
        guard fdx != 0 || fdy != 0 || phase == "e" || phase == "E" || phase == "b" else { return }
        Task { await aufnahme.rollt(wid) }
        sendeText(String(format: "ZEIGER radf %u %d %d %.3f %.3f %@ %.0f", wid, Int(ziel.x), Int(ziel.y),
                         fdx, fdy, phase, Date().timeIntervalSince1970 * 1000))
        DispatchQueue.main.async { hoverAbgleichen() }
    }
}

final class Tafel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
    /// TEMPORARY_VISIBILITY: waehrend ein Wechsel auf das Vorschau-Ziel
    /// (Nokis Schreibtisch) laeuft oder Mission Control offen ist, bleibt
    /// die Tafel unsichtbar - egal welcher Pfad gerade alpha 1 setzt. Die
    /// gewuenschte Deckkraft wird gemerkt und beim Aufheben wiederhergestellt.
    /// Ziel, Inhalt und Raum-Mitgliedschaft bleiben unberuehrt.
    var temporaerVerborgen = false {
        didSet { if temporaerVerborgen != oldValue { super.alphaValue = temporaerVerborgen ? 0 : gewuenschtAlpha } }
    }
    private var gewuenschtAlpha: CGFloat = 1
    override var alphaValue: CGFloat {
        get { super.alphaValue }
        set { gewuenschtAlpha = newValue; super.alphaValue = temporaerVerborgen ? 0 : newValue }
    }
}

let app = NSApplication.shared          // ScreenCaptureKit verlangt einen GUI-Prozess
// Die Miniatur gehoert einem Hintergrundprogramm: ohne diese Eigenschaft
// setzte macOS jeden NSCursor.set() sofort zurueck (Resize-/Hand-Zeiger).
do {
    typealias FCid = @convention(c) () -> Int32
    typealias FSet = @convention(c) (Int32, Int32, CFString, CFTypeRef) -> Int32
    let alle = dlopen(nil, RTLD_NOW)
    let sky = dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight", RTLD_NOW)
    let c = dlsym(alle, "CGSMainConnectionID") ?? dlsym(sky, "SLSMainConnectionID")
    let f = dlsym(alle, "CGSSetConnectionProperty") ?? dlsym(sky, "SLSSetConnectionProperty")
    if let c, let f {
        let cid = unsafeBitCast(c, to: FCid.self)()
        let r = unsafeBitCast(f, to: FSet.self)(cid, cid, "SetsCursorInBackground" as CFString, kCFBooleanTrue)
        FileHandle.standardError.write("[SCHIRM] SetsCursorInBackground=\(r)\n".data(using: .utf8)!)
    } else {
        FileHandle.standardError.write("[SCHIRM] SetsCursorInBackground unavailable\n".data(using: .utf8)!)
    }
}
app.setActivationPolicy(.accessory)     // kein Dock-Symbol; das Fenster aktiviert nie

/// Frame rate for passively changing content (no input on it right now).
let LIVE_TAKT: Int32 = 6
/// A Dock gesture (Space swipe) is running: every stream at 1 frame/s.
var gestenRuhe = false
/// Capture callbacks during the current gesture (diagnostic, logged once per swipe).
var gestenBilder = 0
var gestenStart = Date()

func tafelBauen(klebend: Bool) -> Tafel {
    let t = Tafel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel],
                  backing: .buffered, defer: false)
    t.isOpaque = false
    t.backgroundColor = .clear
    t.hasShadow = true
    t.hidesOnDeactivate = false
    t.isReleasedWhenClosed = false
    t.level = NSWindow.Level(rawValue: NSWindow.Level.floating.rawValue + 1)
    t.collectionBehavior = klebend
        ? [.canJoinAllSpaces, .ignoresCycle, .fullScreenAuxiliary]
        // `.fullScreenAuxiliary`: ohne dieses Recht bleibt ein Fenster
        // ausserhalb jedes Vollbild-Spaces - auch wenn es dort Mitglied ist.
        : [.ignoresCycle, .fullScreenAuxiliary]
    t.animationBehavior = .none
    // Die fertige Komposition muss den Schreibtischwechsel UEBERLEBEN.
    // Ein Fenster, dessen Ruecklage macOS verwerfen darf, kommt auf dem
    // neuen Schreibtisch leer an - erst das Hintergrundbild, dann tropfen
    // die Fensterbilder nach. Genau das sah der Nutzer als "Wallpaper
    // zuerst". `isOneShot = false` behaelt die Ruecklage, und die Ebenen
    // behalten ihren Inhalt ohne Neuzeichnen.
    t.isOneShot = false
    t.contentView = NSView()
    t.contentView?.wantsLayer = true
    t.contentView?.layerContentsRedrawPolicy = .onSetNeedsDisplay
    return t
}

// ZWEI Fenster, EINE Ansicht. Gemessen auf macOS 26.6:
//  * KLEBEND (von Geburt an CanJoinAllSpaces, nie per CGS angefasst) steht
//    waehrend eines Schreibtischwechsels STILL (Aufnahme st-plain): Nutzer-
//    Schreibtisch -> Nutzer-Schreibtisch ohne jedes Aus/An.
//  * MITGLIED (per CGS genau den Nutzer-Schreibtischen zugeordnet) gleitet
//    mit dem Ziel HEREIN, wenn es auf dem Ausgangs-Space fehlt (abc2):
//    Nokis Schreibtisch -> Nutzer-Schreibtisch, sofort und sauber.
//  * Ein nachtraegliches CanJoinAllSpaces auf einem CGS-Fenster wirkt nicht
//    (cov), ein Fenster in beiden Spaces erscheint erst nach dem Wechsel
//    (mu). Deshalb getrennte Fenster und ein Umhaengen der Ansicht.
/// Die Ebene, die das Standbild der Uebergangsflaeche traegt. Sie gehoert
/// dem klebenden Fenster allein - die lebende Ansicht bleibt unberuehrt.
let deckEbene = CALayer()
let klebe = tafelBauen(klebend: true)       // auf Nutzer-Schreibtischen + Uebergang
let mitglied = tafelBauen(klebend: false)   // traegt dauerhaft die lebende Ansicht
let ansicht = Ansicht(frame: .zero)
// Die lebende Ansicht gehoert ab jetzt dauerhaft dem Mitglieds-Fenster.
// Das klebende bekommt eine eigene, stumme Flaeche fuer das Standbild.
let huelle: NSView = {
    let v = NSView()
    v.wantsLayer = true
    v.layer?.backgroundColor = NSColor.clear.cgColor
    return v
}()
mitglied.contentView = huelle
huelle.addSubview(ansicht)
klebe.contentView = {
    let v = NSView()
    v.wantsLayer = true
    v.layer?.addSublayer(deckEbene)
    return v
}()

var sollRahmen = CGRect.zero        // oben-links-Punkte; .zero = verborgen
var kompaktRahmen = CGRect.zero     // gespeicherte Kompakt-Groesse
var sollSpaces: [UInt64] = []       // Nutzer-Schreibtische (fuer `mitglied`)
var nokiSpace: UInt64 = 0           // Nokis eigener Arbeitsplatz
var ort = "nutzer"                  // nutzer | noki | noki_gewollt
var nokiLinks = false, nokiRechts = false
var sichtbar: Bool { sollRahmen.width >= 2 && sollRahmen.height >= 2 }
/// "Noki Schreibtisch oeffnen": dieselbe Ansicht, dieselben Ebenen, dieselben
/// Aufnahmestroeme und dieselbe Fernbedienung - nur gross genug, um alles zu
/// lesen. Keine zweite Darstellung, die auseinanderlaufen koennte.
var vollAn = false
var naviText = "Noki Schreibtisch öffnen"
var wegGewischt = false             // Geste Richtung Noki: sofort weg

func spaceReihenfolge() -> [UInt64] {
    guard let f_disp = dlsym(cgsGriff, "CGSCopyManagedDisplaySpaces") else { return [] }
    let disps = unsafeBitCast(f_disp, to: (@convention(c) (Int32) -> Unmanaged<CFArray>?).self)(cgsCid)?
        .takeRetainedValue() as? [[String: Any]] ?? []
    var r: [UInt64] = []
    for d in disps {
        if let spaces = d["Spaces"] as? [[String: Any]] {
            for sp in spaces {
                if let id = (sp["ManagedSpaceID"] as? NSNumber)?.uint64Value {
                    let typ = (sp["type"] as? NSNumber)?.int32Value ?? 0
                    if typ != 4 {
                        r.append(id)
                    }
                }
            }
        }
    }
    return r
}

func aktiverSpaceID() -> UInt64 {
    guard let f = dlsym(cgsGriff, "CGSGetActiveSpace") else { return 0 }
    return unsafeBitCast(f, to: (@convention(c) (Int32) -> UInt64).self)(cgsCid)
}

func pruefeNokiRichtung() {
    guard nokiSpace != 0 else { return }
    let reihe = spaceReihenfolge()
    guard let f_act = dlsym(cgsGriff, "CGSGetActiveSpace") else { return }
    let act = unsafeBitCast(f_act, to: (@convention(c) (Int32) -> UInt64).self)(cgsCid)
    guard let iAct = reihe.firstIndex(of: act), let iNoki = reihe.firstIndex(of: nokiSpace) else { return }
    nokiLinks = iNoki < iAct
    nokiRechts = iNoki > iAct
}

func cocoa(_ r: CGRect) -> NSRect {
    let hoehe = NSScreen.screens.first?.frame.height ?? r.maxY
    return NSRect(x: r.minX, y: hoehe - r.maxY, width: r.width, height: r.height)
}

/// Die LEBENDE Ansicht wechselt nie mehr das Fenster.
///
/// Ein NSView, der in ein anderes Fenster umgehaengt wird, bekommt dort
/// einen frischen Ebenenbaum - der fertig komponierte Schreibtisch (Bilder
/// der Fenster) war damit weg und musste sich aus neuen Aufnahmebildern
/// wieder auffuellen. Genau das sah der Nutzer als "erst Hintergrundbild,
/// Sekunden spaeter die Fenster". Die Ansicht lebt deshalb dauerhaft im
/// Mitglieds-Fenster; die Uebergangsflaeche bekommt ein eigenes Fenster mit
/// einem STANDBILD derselben Komposition.
func ansichtEinhaengen() {
    if mitglied.contentView !== huelle { mitglied.contentView = huelle }
    if ansicht.superview !== huelle { huelle.addSubview(ansicht) }
}

/// Das Standbild fuer die Uebergangsflaeche: genau das, was gerade zu sehen
/// ist - kein Nachbau, kein Schwarz.
func deckbildAuffrischen() {
    guard let r = ansicht.bitmapImageRepForCachingDisplay(in: ansicht.bounds) else { return }
    ansicht.cacheDisplay(in: ansicht.bounds, to: r)
    deckEbene.contents = r.cgImage
    deckEbene.contentsGravity = .resizeAspectFill
}

/// Praesentation des Mitglieds-Fensters: bildschirmfest im eigenen
/// Overlay-Raum (kein Schreibtisch). Vorher war es Mitglied aller
/// Nutzer-Schreibtische und wurde nur fuer die Dauer einer Wischgeste in
/// den Overlay-Raum gehoben (festhalten beim Gestenbeginn, Freigabe 0,9 s
/// nach dem Ende). Beides war sichtbar: die ersten Bilder einer echten
/// Geste liefen noch mit dem Schreibtisch, und das Zurueckhaengen danach
/// erzeugte einen Artefakt-Frame. Jetzt wechselt die Miniatur beim Wischen
/// gar keinen Raum mehr. Dass sie auf Nokis Schreibtisch nie erscheint,
/// regelt allein die Logik (`ort == "noki"` -> orderOut, Wischgeste
/// Richtung Noki -> alpha 0 ab Gestenbeginn) - nicht die Mitgliedschaft.
/// `sollSpaces` bleibt nur der Rueckfall, falls kein Overlay-Raum entsteht.
/// Zuletzt BESTAETIGTE Praesentation (Overlay-Raum oder Schreibtische).
var spacesBestaetigt: [UInt64] = []
var praesentationBestaetigt = false

func spacesRichten(versuch: Int = 0) {
    guard mitglied.isVisible else { return }
    let wid = mitglied.windowNumber
    let o = overlayRaumHolen()
    if o == 0 {
        mitgliedschaftRichten(versuch: versuch)
        return
    }
    let alle = spacesAlle(UInt32(wid))
    // Direkt nach orderFront hat das Fenster noch GAR keinen Space; ein
    // Hinzufuegen jetzt ueberschreibt WindowServer gleich danach (gemessen).
    if alle.isEmpty {
        praesentationBestaetigt = false
        if versuch < 20 {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { spacesRichten(versuch: versuch + 1) }
        }
        return
    }
    if alle == [o] {
        praesentationBestaetigt = true
        spacesBestaetigt = [o]
        if mitglied.alphaValue < 1 && !wegGewischt && ort != "noki" { mitglied.alphaValue = 1 }
        return
    }
    praesentationBestaetigt = false
    // Erst in den Overlay-Raum, dann aus den Schreibtischen - zu keinem
    // Zeitpunkt auf gar keinem Raum.
    if !alle.contains(o) { fensterSpaces("CGSAddWindowsToSpaces", wid, [o]) }
    let verwaltet = spacesVon(UInt32(wid)).filter { $0 != o }
    fensterSpaces("CGSRemoveWindowsFromSpaces", wid, verwaltet)
    let jetzt = spacesAlle(UInt32(wid))
    if jetzt == [o] {
        praesentationBestaetigt = true
        spacesBestaetigt = [o]
        log("praesentation overlay=\(o)")
    } else if versuch < 24 {
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { spacesRichten(versuch: 24) }
    }
}

/// Rueckfall ohne Overlay-Raum: Mitgliedschaft genau auf die
/// Nutzer-Schreibtische (alte Praesentation).
func mitgliedschaftRichten(versuch: Int = 0) {
    guard mitglied.isVisible, !sollSpaces.isEmpty else { return }
    let ist = Set(spacesVon(UInt32(mitglied.windowNumber)))
    let soll = Set(sollSpaces)
    if ist.isEmpty {
        if versuch < 20 {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { mitgliedschaftRichten(versuch: versuch + 1) }
        }
        return
    }
    if ist == soll {
        spacesBestaetigt = sollSpaces
        praesentationBestaetigt = true
        if mitglied.alphaValue < 1 && !wegGewischt { mitglied.alphaValue = 1 }
        return
    }
    spacesBestaetigt = []
    praesentationBestaetigt = false
    fensterSpaces("CGSAddWindowsToSpaces", mitglied.windowNumber, Array(soll.subtracting(ist)))
    fensterSpaces("CGSRemoveWindowsFromSpaces", mitglied.windowNumber, Array(ist.subtracting(soll)))
    if versuch < 24 {
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { mitgliedschaftRichten(versuch: 24) }
    }
    if Set(spacesVon(UInt32(mitglied.windowNumber))) == soll { mitglied.alphaValue = 1 }
}

// ---------------------------------------------------------------------------
//  Overlay-Raum: bildschirmfest, kein Schreibtisch
// ---------------------------------------------------------------------------
// Derselbe Weg wie das Shortcut-9-Panel: ein eigener CGS-Raum auf absoluter
// Ebene 0. Die Wischanimation traegt nur verwaltete Schreibtische; Fenster
// in diesem Raum bleiben stehen, waehrend der Schreibtisch darunter gleitet.
var overlayRaum: UInt64 = 0
func overlayRaumHolen() -> UInt64 {
    if overlayRaum != 0 { return overlayRaum }
    guard let fc = dlsym(cgsGriff, "CGSSpaceCreate"), let fl = dlsym(cgsGriff, "CGSSpaceSetAbsoluteLevel"),
          let fs = dlsym(cgsGriff, "CGSShowSpaces") else { return 0 }
    let sid = unsafeBitCast(fc, to: (@convention(c) (Int32, Int32, CFDictionary?) -> UInt64).self)(cgsCid, 1, nil)
    guard sid != 0 else { return 0 }
    _ = unsafeBitCast(fl, to: (@convention(c) (Int32, UInt64, Int32) -> Int32).self)(cgsCid, sid, 0)
    unsafeBitCast(fs, to: (@convention(c) (Int32, CFArray) -> Void).self)(cgsCid, [NSNumber(value: sid)] as CFArray)
    overlayRaum = sid
    return sid
}
/// Mission Control skaliert beim Oeffnen JEDEN gezeigten Raum - auch diesen
/// eigenen. Beim Schliessen setzt das Dock nur seine verwalteten Raeume
/// zurueck: der Overlay-Raum blieb verkleinert und verschoben (gemessen:
/// Tafel 706x483 bei 7,451 erschien als 637x436 bei 79,453; Fenster-Transform
/// unveraendert). Abhilfe an der Ursache: frischer, unverzerrter Raum; die
/// Tafel zieht um (sie ist in Mission Control ohnehin unsichtbar), der alte
/// Raum wird aufgeloest.
func overlayRaumErneuern(_ grund: String) {
    let alt = overlayRaum
    guard alt != 0 else { return }
    overlayRaum = 0
    let neu = overlayRaumHolen()
    guard neu != 0 else { overlayRaum = alt; return }
    let wid = mitglied.windowNumber
    if spacesAlle(UInt32(wid)).contains(alt) {
        fensterSpaces("CGSAddWindowsToSpaces", wid, [neu])
        fensterSpaces("CGSRemoveWindowsFromSpaces", wid, [alt])
    }
    if let fd = dlsym(cgsGriff, "CGSSpaceDestroy") {
        unsafeBitCast(fd, to: (@convention(c) (Int32, UInt64) -> Void).self)(cgsCid, alt)
    }
    praesentationBestaetigt = false
    spacesRichten()
    log("overlay erneuert (\(grund)): \(alt) -> \(neu) fenster=\(spacesAlle(UInt32(wid)))")
}

/// Vollstaendige Schichtmessung der Miniatur (ohne etwas zu veraendern):
/// AppKit-Fenster, Inhalt, Ansicht, WindowServer-Bounds, Fenster- und
/// Raum-Transform, Bildschirm, Spaces, Ort. Eine Zeile ins Laufzeitlog.
func miniaturMessen(_ grund: String) {
    let r = { (x: CGRect) in String(format: "%.1f,%.1f %.1fx%.1f", x.minX, x.minY, x.width, x.height) }
    let tf = { (t: CGAffineTransform) in String(format: "[%.4f %.4f %.4f %.4f %.1f %.1f]", t.a, t.b, t.c, t.d, t.tx, t.ty) }
    let wid = mitglied.windowNumber
    var cg = "keins"
    if let l = CGWindowListCopyWindowInfo([.optionIncludingWindow], CGWindowID(wid)) as? [[String: Any]],
       let b = l.first?[kCGWindowBounds as String] as? NSDictionary, let ist = CGRect(dictionaryRepresentation: b) { cg = r(ist) }
    var wt = "?"
    if let f = dlsym(cgsGriff, "CGSGetWindowTransform") {
        var t = CGAffineTransform.identity
        _ = unsafeBitCast(f, to: (@convention(c) (Int32, UInt32, UnsafeMutablePointer<CGAffineTransform>) -> Int32).self)(cgsCid, UInt32(wid), &t)
        wt = tf(t)
    }
    let h = NSScreen.screens.first?.frame.height ?? 0
    let fr = mitglied.frame
    // Wirksame Abbildung AppKit -> WindowServer (Raum-Transform ist privat
    // nicht stabil lesbar): Massstab und Versatz aus den beiden Rahmen.
    var st = "?"
    if let l = CGWindowListCopyWindowInfo([.optionIncludingWindow], CGWindowID(wid)) as? [[String: Any]],
       let b = l.first?[kCGWindowBounds as String] as? NSDictionary, let ist = CGRect(dictionaryRepresentation: b), fr.width > 0 {
        let sx = ist.width / fr.width, sy = ist.height / fr.height
        st = String(format: "skala=%.4f,%.4f versatz=%.1f,%.1f", sx, sy, ist.minX - fr.minX, ist.minY - (h - fr.maxY))
    }
    let sc = mitglied.screen ?? NSScreen.main
    sendeText("MINIATUR_MESSUNG grund=\(grund) soll=\(r(sollRahmen)) kompakt=\(r(kompaktRahmen))"
        + " nswindow=\(r(fr)) nswindow_tl=\(r(CGRect(x: fr.minX, y: h - fr.maxY, width: fr.width, height: fr.height)))"
        + " content=\(r(mitglied.contentView?.frame ?? .zero)) ansicht=\(r(ansicht.frame)) tafel=\(r(sichtbareTafel))"
        + " cgwindow=\(cg) fenster_tf=\(wt) raum=\(overlayRaum) wirksam=\(st)"
        + " scale=\(sc?.backingScaleFactor ?? 0) screen=\(r(sc?.frame ?? .zero)) visible=\(r(sc?.visibleFrame ?? .zero))"
        + " aktiv=\(aktiverSpaceID()) fenster_spaces=\(spacesAlle(UInt32(wid))) noki_space=\(nokiSpace) ort=\(ort)"
        + " sichtbar=\(sichtbar) gross=\(ansicht.istGross) voll=\(vollAn) mc=\(mcOffen) isVisible=\(mitglied.isVisible)")
}

/// Zeigt WindowServer die Tafel dort, wo AppKit sie hat? (CG-Rahmen,
/// Ursprung oben links.) Eine Abweichung heisst: der Raum ist verzerrt.
func overlayVerzerrt() -> Bool {
    guard mitglied.isVisible, let l = CGWindowListCopyWindowInfo([.optionIncludingWindow], CGWindowID(mitglied.windowNumber)) as? [[String: Any]],
          let b = l.first?[kCGWindowBounds as String] as? NSDictionary, let r = CGRect(dictionaryRepresentation: b),
          let h = NSScreen.screens.first?.frame.height else { return false }
    let f = mitglied.frame
    let soll = CGRect(x: f.minX, y: h - f.maxY, width: f.width, height: f.height)
    return abs(r.minX - soll.minX) > 1.5 || abs(r.minY - soll.minY) > 1.5
        || abs(r.width - soll.width) > 1.5 || abs(r.height - soll.height) > 1.5
}

/// THE position invariant of the Miniatur - event-driven only (no timer).
/// Single owner of the frame: Rust's canonical corner -> `kompaktRahmen` ->
/// `sollRahmen` -> `anzeigen`/`tafelSetzen`. The only foreign writer
/// measured is the Dock: Mission Control - also a started-and-cancelled MC
/// swipe, which sends NO Expose notification - scales and shifts every
/// shown space, Noki's overlay space included, and never resets it
/// (measured 2026-10-02: 706x483 at 7,451 shown as 637x436 at 79,453 =
/// "slightly up and right"). WindowServer's frame must equal ours (+-1 pt);
/// otherwise log MINIATUR_POSITION_DRIFT and renew the space at once.
func lageSichern(_ grund: String) {
    guard mitglied.isVisible, !mcOffen, overlayRaum != 0 else { return }
    guard let l = CGWindowListCopyWindowInfo([.optionIncludingWindow], CGWindowID(mitglied.windowNumber)) as? [[String: Any]],
          let b = l.first?[kCGWindowBounds as String] as? NSDictionary, let ist = CGRect(dictionaryRepresentation: b),
          let h = NSScreen.screens.first?.frame.height else { return }
    let f = mitglied.frame
    let soll = CGRect(x: f.minX, y: h - f.maxY, width: f.width, height: f.height)
    guard abs(ist.minX - soll.minX) > 1 || abs(ist.minY - soll.minY) > 1
        || abs(ist.width - soll.width) > 1 || abs(ist.height - soll.height) > 1 else { return }
    let d = { (r: CGRect) in "\(Int(r.minX)),\(Int(r.minY)) \(Int(r.width))x\(Int(r.height))" }
    sendeText("MINIATUR_POSITION_DRIFT grund=\(grund) ist=\(d(ist)) soll=\(d(soll)) screen=\(schreibtischAnzeige)")
    miniaturMessen("drift-vorher: \(grund)")
    overlayRaumErneuern("drift: \(grund)")
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) {
        miniaturMessen("drift-nachher: \(grund)")
        // Sicherheitsnetz, nicht die Korrektur: gemessen hilft ein neuer Raum
        // gegen den Aufwach-Zoom NICHT (die Verbindung behaelt ihn). Ein bei
        // wachem Display frisch gestarteter Helfer ist korrekt. Hoechstens
        // einmal je 2 Minuten, nie waehrend Mission Control.
        guard overlayVerzerrt(), !mcOffen, CGDisplayIsAsleep(CGMainDisplayID()) == 0 else { return }
        let marke = (NSTemporaryDirectory() as NSString).appendingPathComponent("noki-schirm-eskalation")
        let alt = (try? String(contentsOfFile: marke, encoding: .utf8)).flatMap { Double($0) } ?? 0
        let jetzt = Date().timeIntervalSince1970
        guard jetzt - alt > 120 else { return }
        try? String(jetzt).write(toFile: marke, atomically: true, encoding: .utf8)
        sendeText("MINIATUR_MESSUNG grund=eskalation-neustart (Raum-Erneuerung unwirksam)")
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) { exit(0) }
    }
}

func spacesAlle(_ id: UInt32) -> [UInt64] {
    guard let b = dlsym(cgsGriff, "CGSCopySpacesForWindows") else { return [] }
    let sp = unsafeBitCast(b, to: CgsSpacesFuerFenster.self)(cgsCid, 15, [NSNumber(value: id)] as CFArray)?
        .takeRetainedValue() as? [NSNumber] ?? []
    return sp.map { $0.uint64Value }
}

func grossRahmen(_ r: CGRect) -> CGRect {
    guard r.width >= 2 && r.height >= 2 else { return r }
    let ar = (schreibtisch.height > 0 && schreibtisch.width > 0) ? (schreibtisch.height / schreibtisch.width) : 0.625
    let gw = min(860, max(420, round(r.width * 2.0)))
    let gh = round((gw * ar) + FUSS)
    let gx = r.minX
    let gy = r.maxY - gh
    return CGRect(x: gx, y: gy, width: gw, height: gh)
}

var vermessenItem: DispatchWorkItem?
var hoverZeit = Date.distantPast

var fernRunde = 0

/// DIE verbindliche Groessenentscheidung der Miniatur.
///
/// Sich allein auf mouseExited zu verlassen reichte nicht: ueber einen
/// Schreibtischwechsel oder einen Fernvorgang verliert macOS dieses Ereignis,
/// und die Miniatur blieb gross stehen. Hier zaehlt nur, wo der Zeiger
/// WIRKLICH steht. Ist gerade eine Maustaste unten, wird nicht eingeklappt
/// (ein Klick ist noch im Gange) - der naechste Abgleich holt es nach.
/// Laeuft ein verdeckter Fernvorgang? Dann steht der Bildschirm kurz auf
/// Nokis Schreibtisch, die Blende nimmt jeden physischen Klick (Isolation),
/// und das Panel sieht nichts. Der Mitlese-Tap uebernimmt dann Druecke und
/// Rollen ueber der Miniatur, damit kein bewusster Klick verloren geht.
var fernLaeuft = false
var fernNachlaufBis = Date.distantPast
/// Zeitstempel der Ereignisse, die der Tap schon uebernommen hat - das
/// Panel ignoriert genau diese (kein Doppelklick an der Grenze).
var fernTapZeiten: [TimeInterval] = []

func fernTapHatte(_ t: TimeInterval) -> Bool {
    fernTapZeiten.contains { abs($0 - t) < 0.000_5 }
}

/// Tap-Ereignis (globale Punkte, oben-links) -> Ansichtspunkt, nur wenn es
/// waehrend eines Fernvorgangs IN der sichtbaren Miniatur liegt.
/// Ein vom Tap begonnener Druck endet auch im Tap - egal, wie lange er
/// dauert. Sonst ginge sein Loslassen an ein Panel, das den Druck nie sah.
var fernTapDruckOffen = false

func fernTapPunkt(_ lage: CGPoint, erzwingen: Bool = false) -> NSPoint? {
    guard erzwingen || fernLaeuft || Date() < fernNachlaufBis else { return nil }
    guard sichtbar, !ansicht.abgedeckt, ort != "noki", let w = ansicht.window else { return nil }
    let hoehe = NSScreen.screens.first?.frame.height ?? 0
    let schirm = NSPoint(x: lage.x, y: hoehe - lage.y)
    guard cocoa(sollRahmen).contains(schirm) else { return nil }
    return ansicht.convert(w.convertPoint(fromScreen: schirm), from: nil)
}

func fernTapEreignis(_ typ: CGEventType, _ ev: CGEvent) {
    let offen = typ == .leftMouseUp && fernTapDruckOffen
    if typ == .leftMouseUp { fernTapDruckOffen = false }
    guard let p = fernTapPunkt(ev.location, erzwingen: offen), let ne = NSEvent(cgEvent: ev) else {
        // Losgelassen ausserhalb: der Druck ist trotzdem zu Ende.
        if offen { ansicht.gedruecktesFernziel = nil }
        return
    }
    fernTapZeiten.append(ne.timestamp)
    if fernTapZeiten.count > 16 { fernTapZeiten.removeFirst(fernTapZeiten.count - 16) }
    switch typ {
    case .leftMouseDown:
        fernTapDruckOffen = true
        ansicht.druckBeginn(p, klicks: Int(ev.getIntegerValueField(.mouseEventClickState)))
    case .leftMouseUp: ansicht.druckEnde(p)
    case .scrollWheel: ansicht.rollen(p, ne)
    default: break
    }
}

/// Rahmen der Vollansicht (oben-links-Punkte): der sichtbare Bereich des
/// Nutzer-Bildschirms, im Seitenverhaeltnis von Noki Schreibtisch.
func vollRahmen() -> CGRect {
    let primaer = NSScreen.screens.first
    let hoehe = primaer?.frame.height ?? 0
    let mitte = NSPoint(x: cocoa(kompaktRahmen).midX, y: cocoa(kompaktRahmen).midY)
    let schirm = NSScreen.screens.first(where: { $0.frame.contains(mitte) }) ?? primaer
    guard let vis = schirm?.visibleFrame.insetBy(dx: 8, dy: 8), vis.width > 100 else { return kompaktRahmen }
    let ar = (schreibtisch.width > 0) ? schreibtisch.height / schreibtisch.width : 0.65
    let w = floor(min(vis.width, (vis.height - FUSS) / ar))
    let h = floor(w * ar + FUSS)
    let x = vis.minX + (vis.width - w) / 2
    let yCocoa = vis.minY + (vis.height - h) / 2
    return CGRect(x: x, y: hoehe - (yCocoa + h), width: w, height: h)
}

func vollSetzen(_ an: Bool) {
    guard vollAn != an else { if an { sendeText("VOLL an") }; return }
    guard !an || kompaktRahmen.width >= 2 else { sendeText("VOLL aus"); return }
    vollAn = an
    ansicht.navi.titel = an ? "Schließen" : naviText
    ansicht.navi.pfeil = !an
    if an {
        sollRahmen = vollRahmen()
    } else {
        ansicht.istGross = false
        sollRahmen = kompaktRahmen
    }
    anzeigen(animiert: true)
    ansicht.needsLayout = true
    sendeText(an ? "VOLL an" : "VOLL aus")
    vermessenItem?.cancel()
    let item = DispatchWorkItem { Task { await aufnahme.neuVermessen() } }
    vermessenItem = item
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.32, execute: item)
    if !an { DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { hoverAbgleichen() } }
}

/// Groessenstabilitaet (2026-10-03): LARGE gehoert NUR dem Zeiger, der
/// gerade darauf steht. Gemessen blieb die Miniatur bis 267 s gross ueber
/// Schreibtischwechsel und Mission Control hinweg (hoverAbgleichen kehrte
/// bei verborgen/abgedeckt frueh zurueck) - danach erschien sie zu gross und
/// passte unten nicht mehr. Verborgen, abgedeckt, Mission Control oder ein
/// Ortswechsel setzen den Zustand ohne Animation auf das kanonische COMPACT
/// zurueck; wachsen kann sie nur, wenn der Zeiger wieder IN COMPACT steht.
func kanonischKompakt(_ grund: String) {
    guard !vollAn, ansicht.istGross else { return }
    ansicht.istGross = false
    sollRahmen = kompaktRahmen
    if sichtbar && !ansicht.abgedeckt && ort != "noki" { anzeigen(animiert: false) }
    sendeText("HOVER kompakt")
    log("groesse kanonisch kompakt grund=\(grund)")
}

func hoverAbgleichen() {
    // Die Vollansicht gehoert dem ausdruecklichen Oeffnen/Schliessen, nie
    // der Zeigerlage.
    guard !vollAn, !menueOffen else { return }
    if !sichtbar || ansicht.abgedeckt || ort == "noki" || mcOffen { kanonischKompakt("verborgen"); return }
    if NSEvent.pressedMouseButtons != 0 { return }
    let maus = NSEvent.mouseLocation
    let kompakt = cocoa(kompaktRahmen)
    let gross = cocoa(grossRahmen(kompaktRahmen))
    let sollGross = kompakt.contains(maus) || (ansicht.istGross && gross.contains(maus))
    hoverSetzen(gross: sollGross)
}

func hoverSetzen(gross: Bool) {
    guard !vollAn else { return }
    guard !ansicht.abgedeckt, sichtbar else { return }
    // Nicht nur dem Merker trauen: gemessen stand `istGross` nach dem Start
    // auf "gross", waehrend die Ansicht kompakt war - dann vergroesserte
    // Hover nie, und Klicks auf die (erwarteten) grossen Knoepfe gingen
    // daneben. Massgeblich ist, was tatsaechlich gezeigt wird.
    let soll = cocoa(gross ? grossRahmen(kompaktRahmen) : kompaktRahmen)
    let gezeigt = sichtbareTafel.width >= 2 ? sichtbareTafel : cocoa(sollRahmen)
    guard ansicht.istGross != gross || abs(gezeigt.width - soll.width) > 2 || abs(gezeigt.height - soll.height) > 2 else { return }
    ansicht.istGross = gross
    hoverZeit = Date()
    let alt = sollRahmen.size
    sollRahmen = gross ? grossRahmen(kompaktRahmen) : kompaktRahmen
    anzeigen(animiert: true)
    sendeText(gross ? "HOVER gross" : "HOVER kompakt")
    // Die Aufloesung der Aufnahme wird erst NACH der Groessenkurve
    // nachgezogen - und nur einmal je Ruhepause. Mitten in der Animation
    // konfigurierte sie die Stroeme neu, und genau das war das Ruckeln.
    if alt != sollRahmen.size && sollRahmen.width >= 2 {
        // Hochstufen erst nach kurzem Verweilen, Herunterstufen erst nach
        // 8 s: gemessen konfigurierte jedes schnelle Rein/Raus alle Stroeme
        // neu (WindowServer/ScreenCaptureKit renderten alles neu). Bis dahin
        // zeigt die Ansicht die vorhandene Textur (Verkleinern = Mipmaps).
        vermessenItem?.cancel()
        let item = DispatchWorkItem { Task { await aufnahme.neuVermessen() } }
        vermessenItem = item
        DispatchQueue.main.asyncAfter(deadline: .now() + (gross ? 0.6 : 8.0), execute: item)
    }
}

/// Die tatsaechliche Lage an Noki melden (Tastatur-Isolation: ein Klick
/// ausserhalb beendet das Tippen, einer innerhalb entscheidet der Helfer).
var gemeldeterRahmen = CGRect(x: -1, y: -1, width: 0, height: 0)
func rahmenMelden() {
    let r = sichtbar ? sollRahmen : .zero
    guard r != gemeldeterRahmen else { return }
    gemeldeterRahmen = r
    sendeText("RAHMEN \(Int(r.minX)) \(Int(r.minY)) \(Int(r.width)) \(Int(r.height))")
}

/// Erst sichtbar, wenn die ERSTE Komposition vollstaendig steht. Vorher
/// zeigte der Start sofort das Hintergrundbild und liess die Fenster
/// einzeln hereinspringen (Nutzer: "erst Hintergrund, dann Fenster").
/// Spaetestens nach 1,5 s wird trotzdem gezeigt (letzte gueltige Bilder).
var ersteKompositionDa = false
func ersteKompositionFertig(_ grund: String) {
    guard !ersteKompositionDa else { return }
    if grund == "frist" && !startIds.isEmpty && ansicht.fensterEbenen.isEmpty {
        log("erste komposition frist ignoriert: fenster noch nicht geladen")
        return
    }
    ersteKompositionDa = true
    log("erste komposition \(grund)")
    anzeigen()
}

/// Label + app bar for the next retarget, applied at its atomic swap.
var wartendeZielMeta: (navi: String, leiste: [[String: Any]])?
/// A Miniatur target switch is in flight (new composition not swapped in
/// yet). Label changes wait for the swap: never "Desktop 1" over the still
/// shown picture of Desktop 5 (seen as "Desktop 1 is empty, then fills").
@MainActor var zielWechselLaeuft = false
@MainActor var wartendeNavi: String?

@MainActor func zielMetaAnwenden() {
    if let n = wartendeNavi {
        wartendeNavi = nil
        naviText = n
        if !vollAn { ansicht.navi.titel = naviText }
        ansicht.needsLayout = true
    }
    guard let m = wartendeZielMeta else { return }
    wartendeZielMeta = nil
    if !m.navi.isEmpty {
        naviText = m.navi
        if !vollAn { ansicht.navi.titel = naviText }
    }
    ansicht.leisteSetzen(m.leiste)
    ansicht.needsLayout = true
}

/// Die eine Stelle, die aus Rahmen + Ort ein Bild macht.
func anzeigen(animiert: Bool = false) {
    defer { rahmenMelden() }
    // TEMPORARY audit: nothing but COMPACT and LARGE may ever be shown.
    if !vollAn && kompaktRahmen.width >= 2 && sollRahmen.width > grossRahmen(kompaktRahmen).width + 2 {
        sendeText("GIANT soll=\(Int(sollRahmen.width))x\(Int(sollRahmen.height)) gross=\(Int(grossRahmen(kompaktRahmen).width))")
    }
    guard !ansicht.abgedeckt else { return }
    guard ersteKompositionDa || !sichtbar else { return }
    guard sichtbar else {
        ansicht.hoverWorkItem?.cancel()
        ansicht.hoverWorkItem = nil
        klebe.orderOut(nil); mitglied.orderOut(nil)
        return
    }
    let ziel = cocoa(sollRahmen)
    if ort == "noki" {
        // P0: PHYSICAL_CURRENT_SPACE == PREVIEW_TARGET_SPACE
        // Miniatur must be completely hidden on this Space.
        // Never show Desktop inside the same Desktop (recursion).
        ansicht.hoverWorkItem?.cancel()
        ansicht.hoverWorkItem = nil
        mitglied.alphaValue = 0
        klebe.orderOut(nil)
        mitglied.orderOut(nil)
        spacesRichten()
        return
    }
    // Nutzer-Schreibtisch.
    //
    // Frueher lag hier das KLEBENDE Fenster (canJoinAllSpaces). Damit war
    // die Miniatur auch auf Nokis Schreibtisch vorhanden - sie wurde dort
    // nur schnell auf alpha 0 gesetzt. Genau dieses Wettrennen sah der
    // Nutzer als kurzes Aufblitzen. Ein Fenster, das auf einem Schreibtisch
    // GAR NICHT Mitglied ist, kann dort auch nicht aufblitzen. Deshalb gilt
    // jetzt auch hier das Mitglieds-Fenster mit ausdruecklicher
    // Mitgliedschaft (alle Nutzer-Schreibtische, nie Nokis).
    // Das klebende bleibt nur noch fuer die Uebergangsflaeche beim Klick,
    // die den Weg ueber fremde Schreibtische abdecken muss.
    if !sollSpaces.isEmpty || overlayRaumHolen() != 0 {
        if !wegGewischt && sichtbar && mitglied.alphaValue < 1 {
            mitglied.alphaValue = 1
        }
        if ansicht.window !== mitglied {
            tafelSetzen(ziel, animiert: false)
            ansichtEinhaengen()
            mitglied.alphaValue = 1
            mitglied.orderFrontRegardless()
            klebe.orderOut(nil)
            spacesRichten()
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { lageSichern("einblenden") }
            return
        }
        if !mitglied.isVisible {
            tafelSetzen(ziel, animiert: false)
            mitglied.alphaValue = 1
            mitglied.orderFrontRegardless()
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { lageSichern("einblenden") }
        } else if animiert && ansicht.frame.size != ziel.size {
            tafelSetzen(ziel, animiert: true)
            // Reine Groessenaenderung (Hover): die Space-Mitgliedschaft ist
            // unveraendert. Gemessen: die synchrone Space-Abfrage hier hielt
            // bei ausgelastetem WindowServer den Hauptfaden - und damit den
            // Beginn der Hover-Animation - bis zu 1,9 s auf.
            if praesentationBestaetigt { return }
        } else {
            tafelSetzen(ziel, animiert: false)
        }
        spacesRichten()
        return
    }
    if !wegGewischt && sichtbar && mitglied.alphaValue < 1 {
        mitglied.alphaValue = 1
    }
    if !mitglied.isVisible {
        tafelSetzen(ziel, animiert: false)
        mitglied.alphaValue = wegGewischt ? 0 : 1
        mitglied.orderFrontRegardless()
        return
    }
    tafelSetzen(ziel, animiert: animiert && ansicht.frame.size != ziel.size)
}

/// Das Mitglieds-Fenster hat einen FESTEN Rahmen (die Huelle: grosse Lage,
/// die die kompakte enthaelt). COMPACT <-> LARGE bewegt nur die Ansicht
/// darin. Gemessen: jede Stufe einer Fenster-Groessenanimation verlangt von
/// WindowServer eine echte Fensterneugroesse samt Puffer; bei ausgelastetem
/// WindowServer wurde die 0,24-s-Kurve bis zu 0,9 s spaeter sichtbar. Eine
/// Ebenenbewegung im festen Fenster ist fuer WindowServer nur Zusammensetzen.
/// Ausserhalb der sichtbaren Ansicht laesst das Fenster Maus durch.
var sichtbareTafel = CGRect.zero   // Cocoa-Bildschirmkoordinaten
func tafelSetzen(_ ziel: CGRect, animiert: Bool) {
    let gross = kompaktRahmen.width >= 2 ? cocoa(grossRahmen(kompaktRahmen)) : ziel
    let altSichtbar = (ansicht.superview === huelle && mitglied.isVisible && ansicht.frame.width >= 2)
        ? mitglied.convertToScreen(ansicht.frame) : ziel
    // Waehrend einer Animation muss die Huelle BEIDE Lagen enthalten.
    let huelleRect = (animiert ? gross.union(ziel).union(altSichtbar) : gross.union(ziel)).integral
    if mitglied.frame != huelleRect {
        mitglied.setFrame(huelleRect, display: false)
        huelle.frame = CGRect(origin: .zero, size: huelleRect.size)
        ansicht.frame = CGRect(x: altSichtbar.minX - huelleRect.minX, y: altSichtbar.minY - huelleRect.minY,
                               width: altSichtbar.width, height: altSichtbar.height)
    }
    let innen = CGRect(x: ziel.minX - huelleRect.minX, y: ziel.minY - huelleRect.minY,
                       width: ziel.width, height: ziel.height)
    sichtbareTafel = ziel
    durchlassRichten(NSEvent.mouseLocation)
    if animiert && ansicht.frame != innen {
        let t0 = Date()
        NSAnimationContext.runAnimationGroup({ c in
            c.duration = 0.24
            c.timingFunction = CAMediaTimingFunction(controlPoints: 0.2, 0.72, 0.25, 1)
            c.allowsImplicitAnimation = true
            ansicht.animator().frame = innen
        }, completionHandler: {
            sendeText(String(format: "ANIM %.0f %.0f", t0.timeIntervalSince1970 * 1000, Date().timeIntervalSince(t0) * 1000))
            // Danach die Huelle auf das Noetige zuruecknehmen (z. B. nach der Vollansicht).
            if sichtbareTafel == ziel { tafelSetzen(ziel, animiert: false) }
        })
    } else if ansicht.frame != innen {
        ansicht.frame = innen
    }
}

/// Mausereignisse nur innerhalb der sichtbaren Ansicht annehmen.
func durchlassRichten(_ maus: NSPoint) {
    let drin = sichtbareTafel.contains(maus) || ansicht.gedruecktesFernziel != nil || ansicht.zieht
    if mitglied.ignoresMouseEvents == drin { mitglied.ignoresMouseEvents = !drin }
}

enum TransitionState: String {
    case stableOffNoki = "STABLE_OFF_NOKI"
    case transitioningToNoki = "TRANSITIONING_TO_NOKI"
    case stableOnNoki = "STABLE_ON_NOKI"
    case transitioningAway = "TRANSITIONING_AWAY"
    case cancelledTransition = "CANCELLED_TRANSITION"
    case recoveringPreview = "RECOVERING_PREVIEW"
}
var uebergangZustand: TransitionState = .stableOffNoki
var transitionEpoch: UInt64 = 0

func naechsteTransitionEpoch() -> UInt64 {
    transitionEpoch &+= 1
    return transitionEpoch
}

func transitionSetzen(_ neu: TransitionState) {
    uebergangZustand = neu
    log("transition \(neu.rawValue) epoch=\(transitionEpoch)")
    sendeText("TRANSITION \(neu.rawValue)")
}

/// Eine Geste oder ein Ctrl+Pfeil beginnt. Fuehrt sie auf Nokis
/// Schreibtisch, verschwindet die klebende Miniatur SOFORT - bevor von
/// Nokis Schreibtisch auch nur ein Streifen sichtbar ist. Fenster-Alpha ist
/// eine Sache von WindowServer, kein Neuzeichnen.
var wischWatchdog: DispatchWorkItem?

/// Fuehrt DIESE Geste direkt auf Nokis Schreibtisch? Eine Wischgeste geht
/// genau einen Space weiter; massgeblich ist also der unmittelbare Nachbar
/// in Wischrichtung (Vollbild-Spaces mitgezaehlt). Vorher genuegte "Noki
/// liegt irgendwo in dieser Richtung" - dann verschwand die Miniatur auch
/// beim Wischen zwischen zwei Nutzer-Schreibtischen und kam erst am Ende
/// wieder.
func richtungNoki(rechts: Bool) -> Bool {
    guard nokiSpace != 0, let f = dlsym(cgsGriff, "CGSCopyManagedDisplaySpaces") else {
        return (rechts && nokiRechts) || (!rechts && nokiLinks)
    }
    let disps = unsafeBitCast(f, to: (@convention(c) (Int32) -> Unmanaged<CFArray>?).self)(cgsCid)?
        .takeRetainedValue() as? [[String: Any]] ?? []
    for d in disps {
        let ids = (d["Spaces"] as? [[String: Any]] ?? []).compactMap { ($0["ManagedSpaceID"] as? NSNumber)?.uint64Value }
        guard let akt = ((d["Current Space"] as? [String: Any])?["ManagedSpaceID"] as? NSNumber)?.uint64Value,
              let i = ids.firstIndex(of: akt) else { continue }
        let j = rechts ? i + 1 : i - 1
        return j >= 0 && j < ids.count && ids[j] == nokiSpace
    }
    return (rechts && nokiRechts) || (!rechts && nokiLinks)
}

func wischBeginn(rechts: Bool) {
    guard ort != "noki", !ansicht.abgedeckt else { return }
    let zuNoki = richtungNoki(rechts: rechts)
    // Weg von Noki: nichts zu tun - die Miniatur liegt ohnehin
    // bildschirmfest im Overlay-Raum und bleibt stehen.
    if zuNoki {
        let epoch = naechsteTransitionEpoch()
        transitionSetzen(.transitioningToNoki)
        if !wegGewischt { log("wisch Richtung Noki -> weg epoch=\(epoch)") }
        wegGewischt = true
        mitglied.alphaValue = 0
        wischWatchdog?.cancel()
        let dog = DispatchWorkItem {
            guard epoch == transitionEpoch else { return }
            guard wegGewischt, ort != "noki" else { return }
            guard let f_act = dlsym(cgsGriff, "CGSGetActiveSpace") else { return }
            let act = unsafeBitCast(f_act, to: (@convention(c) (Int32) -> UInt64).self)(cgsCid)
            if nokiSpace == 0 || act != nokiSpace {
                log("wisch Watchdog: Wischgeste abgelaufen ohne Noki-Space -> zeige wieder")
                transitionSetzen(.cancelledTransition)
                wegGewischt = false
                if sichtbar { mitglied.alphaValue = 1 }
                anzeigen()
                transitionSetzen(.stableOffNoki)
                if sichtbar {
                    Task {
                        await aufnahme.weiter()
                        await aufnahme.frischePruefen()
                    }
                }
            }
        }
        wischWatchdog = dog
        // Only a safety net for a lost gesture END (wischEnde decides): a
        // slow swipe held longer than 1.2 s used to be revealed mid-slide.
        DispatchQueue.main.asyncAfter(deadline: .now() + 4.0, execute: dog)
    }
}

/// Geste zu Ende. Kam kein Wechsel zustande (abgebrochen), kehrt die
/// Miniatur zurueck; kam er zustande, meldet Noki gleich "ort noki".
func wischEnde() {
    wischWatchdog?.cancel()
    wischWatchdog = nil
    guard wegGewischt else { return }
    let epoch = naechsteTransitionEpoch()
    // Revealed ONLY once the switch is decided. CGSGetActiveSpace flips at
    // the END of the release animation (measured); the former fixed check
    // 0.15 s after finger-up ran while the slide onto the target was still
    // going, judged "cancelled" and showed the Miniatur on its own target
    // Desktop for a moment (ghost) until the arrival hid it again.
    let start = aktiverSpace()
    let t0 = Date()
    func pruefen() {
        guard epoch == transitionEpoch else { return }
        guard wegGewischt, ort != "noki" else { return }
        let act = aktiverSpace()
        if nokiSpace != 0 && act == nokiSpace {
            transitionSetzen(.stableOnNoki)
            ortSetzen("noki", links: false, rechts: false)
            return
        }
        // Landed elsewhere, or no switch at all for 1 s (cancelled gesture).
        guard act != start || Date().timeIntervalSince(t0) >= 1.0 else {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.03) { pruefen() }
            return
        }
        transitionSetzen(.cancelledTransition)
        wegGewischt = false
        if sichtbar {
            mitglied.alphaValue = 1
        }
        anzeigen()
        transitionSetzen(.stableOffNoki)
        if sichtbar {
            Task {
                await aufnahme.weiter()
                await aufnahme.frischePruefen()
            }
        }
    }
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { pruefen() }
}

func log(_ t: String) {
    FileHandle.standardError.write(String(format: "[SCHIRM %.3f] ", Date().timeIntervalSince1970.truncatingRemainder(dividingBy: 1000)).data(using: .utf8)! + (t + "\n").data(using: .utf8)!)
}

// ---------------------------------------------------------------------------
//  Direkter Wechsel auf das Vorschau-Ziel: Mission Control, Ctrl+Zahl
// ---------------------------------------------------------------------------
// Der aktive Space (CGSGetActiveSpace) wechselt erst am ENDE der
// Uebergangsanimation (gemessen). Die Miniatur muss aber schon VORHER weg,
// wenn der Weg auf Nokis Schreibtisch fuehrt - sonst zeigt sie dort kurz
// denselben Schreibtisch. Deshalb frueh und nur als voruebergehende
// Sichtbarkeit: Ziel und Praesentation bleiben unveraendert.
func aktiverSpace() -> UInt64 {
    guard let f = dlsym(cgsGriff, "CGSGetActiveSpace") else { return 0 }
    return unsafeBitCast(f, to: (@convention(c) (Int32) -> UInt64).self)(cgsCid)
}
var mcOffen = false
/// Dock-Geste an Noki gemeldet (Shortcut 9 pausiert Live-Aufnahmen).
var gesteGemeldet = false
var gestenRuheEpoche = 0
var direktEpoche = 0
/// Hebt die voruebergehende Verbergung auf, sobald der Wechsel entschieden
/// ist: Landung auf Noki -> `ort noki` uebernimmt (orderOut); sonst zeigen,
/// sobald der Space gewechselt hat oder `ruhe` s ohne Wechsel vergangen sind.
func direktAufloesen(start: UInt64, ruhe: Double, grund: String) {
    direktEpoche += 1
    let e = direktEpoche
    let t0 = Date()
    func pruefen() {
        guard e == direktEpoche, !mcOffen else { return }
        let a = aktiverSpace()
        if nokiSpace != 0 && a == nokiSpace {
            // Angekommen: die Logik (`ort noki`) verbirgt sie dauerhaft.
            if ort != "noki" && ort != "noki_gewollt" && !fernLaeuft && !ansicht.abgedeckt { ortSetzen("noki", links: false, rechts: false) }
            mitglied.temporaerVerborgen = false
            log("direkt \(grund): Ziel erreicht -> verborgen (ort noki)")
            return
        }
        if a != start || Date().timeIntervalSince(t0) >= ruhe {
            mitglied.temporaerVerborgen = false
            log("direkt \(grund): kein Ziel-Space -> sichtbar")
            return
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.03) { pruefen() }
    }
    pruefen()
}
func missionControl(_ offen: Bool) {
    guard offen != mcOffen else { return }
    mcOffen = offen
    if offen { kanonischKompakt("mission_control") }
    // Nokis eigene Navigation (Klick "Zum Schreibtisch") laeuft selbst ueber
    // Mission Control und hat ihre eigene Abdeckung - nicht dazwischenfunken.
    if ansicht.abgedeckt || fernLaeuft {
        direktEpoche += 1
        if !offen && overlayRaum != 0 { overlayRaumErneuern("mission control (Noki-Navigation)") }
        mitglied.temporaerVerborgen = false
        return
    }
    if offen {
        direktEpoche += 1
        guard ort != "noki" else { return }
        mitglied.temporaerVerborgen = true
        log("mission control offen -> voruebergehend verborgen")
    } else {
        // Immer, auch wenn die Tafel gar nicht verborgen war: das Dock hat den
        // Raum skaliert, egal was darin lag.
        if overlayRaum != 0 { overlayRaumErneuern("mission control") }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) {
            lageSichern("nach_mission_control")
        }
        guard mitglied.temporaerVerborgen else { return }
        // Die Rueckfahrt aus Mission Control dauert ~0,3-0,5 s; der aktive
        // Space springt erst an ihrem Ende. Ohne Wechsel: nach 0,7 s zeigen.
        direktAufloesen(start: aktiverSpace(), ruhe: 0.7, grund: "mission control")
    }
}
/// Ctrl+Zahl ("Zu Schreibtisch N wechseln"): nur wenn die Tastenkombination
/// in den Systemeinstellungen AKTIV ist und genau diese Taste belegt.
let zifferTasten: [Int64: Int] = [18: 1, 19: 2, 20: 3, 21: 4, 23: 5, 22: 6, 26: 7, 28: 8, 25: 9, 29: 10]
func schreibtischKuerzel(_ taste: Int64, _ flags: CGEventFlags) -> Int? {
    guard let n = zifferTasten[taste] else { return nil }
    CFPreferencesAppSynchronize("com.apple.symbolichotkeys" as CFString)
    guard let alle = CFPreferencesCopyAppValue("AppleSymbolicHotKeys" as CFString, "com.apple.symbolichotkeys" as CFString) as? [String: Any],
          let eintrag = alle[String(117 + n)] as? [String: Any],
          (eintrag["enabled"] as? NSNumber)?.boolValue == true,
          let wert = eintrag["value"] as? [String: Any],
          let par = wert["parameters"] as? [NSNumber], par.count >= 3,
          par[1].int64Value == taste else { return nil }
    let maske: UInt64 = CGEventFlags.maskControl.rawValue | CGEventFlags.maskAlternate.rawValue
        | CGEventFlags.maskShift.rawValue | CGEventFlags.maskCommand.rawValue
    guard flags.rawValue & maske == par[2].uint64Value & maske else { return nil }
    return n
}
/// Schreibtisch N (nur normale Schreibtische, Vollbild-Spaces zaehlen nicht)
/// des Displays, auf dem der Nutzer steht.
func schreibtischNummer(_ n: Int) -> UInt64? {
    guard let f = dlsym(cgsGriff, "CGSCopyManagedDisplaySpaces") else { return nil }
    let disps = unsafeBitCast(f, to: (@convention(c) (Int32) -> Unmanaged<CFArray>?).self)(cgsCid)?
        .takeRetainedValue() as? [[String: Any]] ?? []
    let akt = aktiverSpace()
    for d in disps {
        let sp = d["Spaces"] as? [[String: Any]] ?? []
        guard sp.contains(where: { ($0["ManagedSpaceID"] as? NSNumber)?.uint64Value == akt }) else { continue }
        let schreibtische = sp.filter { ($0["type"] as? NSNumber)?.intValue == 0 }
            .compactMap { ($0["ManagedSpaceID"] as? NSNumber)?.uint64Value }
        return n <= schreibtische.count ? schreibtische[n - 1] : nil
    }
    return nil
}
func kuerzelWechsel(_ taste: Int64, _ flags: CGEventFlags) {
    guard nokiSpace != 0, ort != "noki", let n = schreibtischKuerzel(taste, flags),
          let ziel = schreibtischNummer(n) else { return }
    let start = aktiverSpace()
    guard ziel == nokiSpace, start != nokiSpace else { return }
    mitglied.temporaerVerborgen = true
    log("ctrl+\(n) -> Nokis Schreibtisch: voruebergehend verborgen")
    // Laeuft der Wechsel nicht an (z. B. anderes Programm faengt die Taste),
    // kommt sie nach 1,2 s zurueck.
    direktAufloesen(start: start, ruhe: 1.2, grund: "ctrl+\(n)")
}

func ortSetzen(_ neu: String, links: Bool, rechts: Bool) {
    log("ort \(neu) L\(links) R\(rechts) weg=\(wegGewischt)")
    wischWatchdog?.cancel()
    wischWatchdog = nil
    nokiLinks = links; nokiRechts = rechts
    let alt = ort
    ort = neu
    if alt != neu { kanonischKompakt("ort \(alt)->\(neu)") }
    if neu != "noki" {
        wegGewischt = false
        if sichtbar { mitglied.alphaValue = 1 }
        transitionSetzen(.stableOffNoki)
        // Capture only for a VISIBLE Miniatur: every "ort nutzer" (each
        // Space change) revived the streams of a hidden/paused Miniatur
        // (measured ~6 frames/s while hidden).
        if sichtbar {
            Task {
                await aufnahme.weiter()
                await aufnahme.frischePruefen()
            }
        }
    } else {
        wegGewischt = false
        transitionSetzen(.stableOnNoki)
    }
    anzeigen()
}

/// Uebergang zu Nokis Schreibtisch: dieselbe echte Komposition, bildschirm-
/// fuellend im KLEBENDEN Fenster - es steht waehrend der Wechsel still und
/// deckt sie ab, ohne je schwarz zu werden.
func abdecken() {
    guard sichtbar else { sendeText("ABGEDECKT"); return }
    ansicht.hoverWorkItem?.cancel()
    ansicht.hoverWorkItem = nil
    if kompaktRahmen != .zero { sollRahmen = kompaktRahmen }
    deckbildAuffrischen()
    ansicht.abgedeckt = true
    klebe.ignoresMouseEvents = true
    if !klebe.isVisible { klebe.setFrame(cocoa(sollRahmen), display: false) }
    klebe.alphaValue = 1
    klebe.orderFrontRegardless()
    mitglied.orderOut(nil)
    let voll = NSScreen.screens.first?.frame ?? klebe.frame
    NSAnimationContext.runAnimationGroup({ c in
        c.duration = 0.16
        c.timingFunction = CAMediaTimingFunction(controlPoints: 0.22, 0.7, 0.2, 1)
        klebe.animator().setFrame(voll, display: true)
        deckEbene.frame = CGRect(origin: .zero, size: voll.size)
    }, completionHandler: {
        ansicht.layer?.cornerRadius = 0
        ansicht.ordnen()
        sendeText("ABGEDECKT")
    })
}

/// Angekommen: die Flaeche blendet ueber dem ECHTEN Schreibtisch aus (beide
/// zeigen dasselbe); danach gilt wieder `anzeigen()` fuer den neuen Ort.
func aufdecken(_ spaces: [UInt64]?) {
    if let s = spaces, !s.isEmpty { sollSpaces = s }
    guard ansicht.abgedeckt else { anzeigen(); sendeText("AUFGEDECKT"); return }
    NSAnimationContext.runAnimationGroup({ c in
        c.duration = 0.14
        klebe.animator().alphaValue = 0
    }, completionHandler: {
        klebe.orderOut(nil)
        ansicht.abgedeckt = false
        ansicht.layer?.cornerRadius = 12
        klebe.ignoresMouseEvents = false
        ansicht.ordnen()
        if ort != "noki" { klebe.alphaValue = 1 }
        anzeigen()
        hoverAbgleichen()
        sendeText("AUFGEDECKT")
    })
}

/// Zeiger ueber der Miniatur? Nur dann gehoert ihr das Aussehen des
/// Mauszeigers; beim Verlassen wird es genau einmal zurueckgegeben.
var zeigerDrin = false

func mausPruefen(loc: CGPoint? = nil) {
    ansicht.hoverWorkItem?.cancel()
    ansicht.hoverWorkItem = nil
    hoverAbgleichen()
    // Der Mitlese-Tap sieht JEDE Bewegung; `mouseMoved` der Ansicht kommt
    // bei einem nicht aktiven Panel nur sporadisch (gemessen: 6 von 20).
    guard let loc, let w = ansicht.window, w.isVisible, !ansicht.abgedeckt else { return }
    let hoehe = NSScreen.screens.first?.frame.height ?? 0
    durchlassRichten(NSPoint(x: loc.x, y: hoehe - loc.y))
    let p = ansicht.convert(w.convertPoint(fromScreen: NSPoint(x: loc.x, y: hoehe - loc.y)), from: nil)
    let drin = ansicht.bounds.contains(p)
    if drin || ansicht.gedruecktesFernziel != nil {
        zeigerDrin = true
        ansicht.letzteMaus = p
        ansicht.zeigerAbgleichen()
    } else if zeigerDrin {
        zeigerDrin = false
        ansicht.ampelHover(0)
        NSCursor.arrow.set()
    }
}

// Gesten und Ctrl+Pfeil mitlesen - NUR lesen, nie abfangen. Eine
// Dock-Wischgeste meldet sich als Ereignistyp 30 (kCGSEventDockControl)
// mit Phase (Feld 132) und Fortschritt (Feld 124, > 0 = nach rechts);
// gemessen mit einer echten Trackpad-Geste. Keine Abfrage, kein Takt.
let wischTap: CFMachPort? = {
    let cb: CGEventTapCallBack = { _, typ, ev, _ in
        if typ == .tapDisabledByTimeout || typ == .tapDisabledByUserInput {
            if let t = wischTap { CGEvent.tapEnable(tap: t, enable: true) }
            return Unmanaged.passUnretained(ev)
        }
        let art = ev.getIntegerValueField(CGEventField(rawValue: 55)!)
        // Legacy kann noch einen begrenzten Fernvorgang melden. REAL_SPACE
        // setzt `fernLaeuft` nicht fuer einen Space-Wechsel: dort bleibt jede
        // Inhaltsinteraktion auf dem aktuellen physischen Space.
        if art == 30 && fernLaeuft { return Unmanaged.passUnretained(ev) }
        if art == 30 {
            let phase = ev.getIntegerValueField(CGEventField(rawValue: 132)!)
            let fort = ev.getDoubleValueField(CGEventField(rawValue: 124)!)
            DispatchQueue.main.async {
                if (phase == 1 || phase == 2) && fort != 0 {
                    if !gesteGemeldet {
                        gesteGemeldet = true; sendeText("WISCH 1")
                        gestenRuheEpoche += 1
                        Task { await aufnahme.gestenRuheSetzen(true) }
                    }
                    wischBeginn(rechts: fort > 0)
                }
                if phase == 4 || phase == 8 {
                    // Any Dock gesture end - incl. a cancelled MC swipe
                    // (no Expose notification): check once the Dock's own
                    // animation has settled.
                    for t in [0.6, 1.4] {
                        DispatchQueue.main.asyncAfter(deadline: .now() + t) { lageSichern("dock_geste_ende") }
                    }
                    if gesteGemeldet {
                        gesteGemeldet = false; sendeText("WISCH 0")
                        // The desktop still glides out ~0.3 s after release.
                        let e = gestenRuheEpoche
                        DispatchQueue.main.asyncAfter(deadline: .now() + 0.35) {
                            guard e == gestenRuheEpoche else { return }
                            Task { await aufnahme.gestenRuheSetzen(false) }
                        }
                    }
                    wischEnde()
                }
            }
        } else if typ == .keyDown && ev.flags.contains(.maskControl) {
            let k = ev.getIntegerValueField(.keyboardEventKeycode)
            if k == 123 || k == 124 {
                DispatchQueue.main.async { wischBeginn(rechts: k == 124); wischEnde() }
            } else if zifferTasten[k] != nil {
                let fl = ev.flags
                DispatchQueue.main.async { kuerzelWechsel(k, fl) }
            }
        } else if typ == .mouseMoved {
            let loc = ev.location
            DispatchQueue.main.async { mausPruefen(loc: loc) }
        } else if typ == .leftMouseDown || typ == .leftMouseUp || typ == .scrollWheel {
            if typ != .scrollWheel && ProcessInfo.processInfo.environment["NOKI_TAP_DIAG"] != nil {
                log("tap \(typ.rawValue) fern=\(fernLaeuft) nach=\(Date() < fernNachlaufBis) punkt=\(fernTapPunkt(ev.location).map { "\($0)" } ?? "-")")
            }
            // Der Callback laeuft auf dem Hauptfaden, VOR der Zustellung ans
            // Panel - deshalb synchron, damit die Dublettensperre greift.
            if let kopie = ev.copy() { fernTapEreignis(typ, kopie) }
        }
        return Unmanaged.passUnretained(ev)
    }
    let maske: CGEventMask = (1 << 30) | (1 << CGEventType.keyDown.rawValue) | (1 << CGEventType.mouseMoved.rawValue)
        | (1 << CGEventType.leftMouseDown.rawValue) | (1 << CGEventType.leftMouseUp.rawValue)
        | (1 << CGEventType.scrollWheel.rawValue)
    guard let t = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .headInsertEventTap,
                                    options: .listenOnly, eventsOfInterest: maske,
                                    callback: cb, userInfo: nil) else { return nil }
    CFRunLoopAddSource(CFRunLoopGetMain(), CFMachPortCreateRunLoopSource(nil, t, 0), .commonModes)
    CGEvent.tapEnable(tap: t, enable: true)
    return t
}()

let _mausGlobal = NSEvent.addGlobalMonitorForEvents(matching: [.mouseMoved]) { _ in
    DispatchQueue.main.async { mausPruefen() }
}
let _mausLokal = NSEvent.addLocalMonitorForEvents(matching: [.mouseMoved]) { ev in
    DispatchQueue.main.async { mausPruefen() }
    return ev
}

// ---------------------------------------------------------------------------
//  Aufnahme
// ---------------------------------------------------------------------------
var bilderZaehler: [UInt32: Int] = [:]

// ---------------------------------------------------------------------
//  Scroll acceptance instruments (debug commands only; idle otherwise)
// ---------------------------------------------------------------------

/// Visual movement of every PUBLISHED frame of one window: a luminance row
/// profile of the central columns, matched against the previous frame's
/// profile. Result = how far the content really moved in the Miniatur
/// (buffer rows), independent of what the page claims.
var messWid: UInt32 = 0
var messProfil: [Float] = []
var messFein: Float = 0
var messDaten: [String] = []
/// Source-frame arrival times (SCK callback queue) and main-thread latency
/// samples > 12 ms, to attribute a presentation gap to source or compositor.
let messSperre = NSLock()
var messQuelle: [Double] = []
var messHaenger: [String] = []
var messTakt: DispatchSourceTimer?
func rollMessung(_ wid: UInt32, _ an: Bool) {
    if an {
        messWid = wid; messProfil = []; messDaten = []
        messSperre.lock(); messQuelle = []; messHaenger = []; messSperre.unlock()
        let t = DispatchSource.makeTimerSource(queue: DispatchQueue.global(qos: .userInteractive))
        t.schedule(deadline: .now(), repeating: .milliseconds(4))
        t.setEventHandler {
            let geplant = Date().timeIntervalSince1970 * 1000
            DispatchQueue.main.async {
                let lag = Date().timeIntervalSince1970 * 1000 - geplant
                if lag > 12 { messSperre.lock(); messHaenger.append(String(format: "%.0f,%.0f", geplant, lag)); messSperre.unlock() }
            }
        }
        t.resume(); messTakt = t
        return
    }
    messTakt?.cancel(); messTakt = nil
    messSperre.lock()
    let quelle = messQuelle.map { String(format: "%.0f", $0) }, haenger = messHaenger
    messSperre.unlock()
    sendeText("ROLLQUELLE \(messWid) " + quelle.suffix(400).joined(separator: ";"))
    sendeText("ROLLHAENGER \(messWid) " + haenger.suffix(200).joined(separator: ";"))
    let w = messWid
    messWid = 0
    // Chunks: one line per 150 samples (the pipe is line based).
    var i = 0
    repeat {
        let teil = messDaten[min(i, messDaten.count)..<min(i + 150, messDaten.count)]
        sendeText("ROLLMESS \(w) \(i) " + teil.joined(separator: ";"))
        i += 150
    } while i < messDaten.count
    sendeText("ROLLMESS_ENDE \(w) \(messDaten.count)")
    messDaten = []
}
func rollMessProbe(_ px: CVPixelBuffer, _ ausschnitt: CGRect) {
    let t = Date().timeIntervalSince1970 * 1000
    CVPixelBufferLockBaseAddress(px, .readOnly)
    defer { CVPixelBufferUnlockBaseAddress(px, .readOnly) }
    guard let basis = CVPixelBufferGetBaseAddress(px) else { return }
    let bpr = CVPixelBufferGetBytesPerRow(px), bw = CVPixelBufferGetWidth(px), bh = CVPixelBufferGetHeight(px)
    let p = basis.assumingMemoryBound(to: UInt8.self)
    // Content rows 18%..96% (below toolbars/mastheads), columns 30%..70%.
    let y0 = Int((ausschnitt.minY + ausschnitt.height * 0.18) * CGFloat(bh))
    let y1 = Int((ausschnitt.minY + ausschnitt.height * 0.96) * CGFloat(bh))
    let x0 = Int((ausschnitt.minX + ausschnitt.width * 0.30) * CGFloat(bw))
    let x1 = Int((ausschnitt.minX + ausschnitt.width * 0.70) * CGFloat(bw))
    guard y1 - y0 > 40, x1 - x0 > 20 else { return }
    var prof = [Float](repeating: 0, count: y1 - y0)
    let schritt = max(1, (x1 - x0) / 48)
    for y in y0..<y1 {
        var sum: Float = 0
        var x = x0
        while x < x1 { sum += Float(p[y * bpr + x * 4 + 1]); x += schritt }
        prof[y - y0] = sum
    }
    var beste = 0
    var guete = 100
    if prof.count == messProfil.count {
        let n = prof.count
        var bestErr = Float.greatestFiniteMagnitude
        let maxS = n / 2
        func fehler(_ s: Int) -> Float {
            // cur[y] == prev[y + s]: s > 0 = content moved up (page down).
            let a = max(0, -s), b = min(n, n - s)
            guard b - a > n / 3 else { return .greatestFiniteMagnitude }
            var err: Float = 0
            var y = a
            while y < b { err += abs(prof[y] - messProfil[y + s]); y += 2 }
            return err / Float((b - a) / 2)
        }
        for s in -maxS...maxS {
            let err = fehler(s)
            if err < bestErr - 0.001 || (abs(err - bestErr) <= 0.001 && abs(s) < abs(beste)) { bestErr = err; beste = s }
        }
        // Sub-row estimate: parabola through the error at best-1/best/best+1.
        let em = fehler(beste - 1), ep = fehler(beste + 1)
        if em < .greatestFiniteMagnitude, ep < .greatestFiniteMagnitude {
            let nenner = em - 2 * bestErr + ep
            if nenner > 0.0001 { messFein = Float(beste) + max(-0.5, min(0.5, 0.5 * (em - ep) / nenner)) } else { messFein = Float(beste) }
        } else { messFein = Float(beste) }
        // Match quality: residual error of the best shift relative to "no
        // shift" (0 = perfect match, 100 = no better than standing still).
        var e0: Float = 0; var y = 0
        while y < n { e0 += abs(prof[y] - messProfil[y]); y += 2 }
        e0 /= Float(max(1, n / 2))
        guete = e0 > 0.001 ? Int(min(100, bestErr / e0 * 100)) : 0
    }
    // Sticky reference: a slow drift of < 1 row per frame accumulates until
    // it is visible, instead of reading as "no motion" frame after frame.
    let fein = prof.count == messProfil.count ? messFein : 0
    if messProfil.count != prof.count || abs(fein) >= 0.5 || guete > 60 { messProfil = prof }
    messDaten.append(String(format: "%.1f,%d,%d,%d,%.2f", t, beste, Int(ausschnitt.height * CGFloat(bh)), guete, abs(fein) >= 0.5 ? fein : 0))
}

/// Synthetic trackpad gesture through the SAME entry a real one uses
/// (`Ansicht.rollen`): NSEvents with scroll/momentum phases at 60 Hz.
func testGeste(_ wid: UInt32, _ rx: CGFloat, _ ry: CGFloat, _ profil: String, _ richtung: Double) {
    guard let l = ansicht.fensterEbenen[wid] else { sendeText("TESTRAD \(wid) kein_fenster"); return }
    let f = l.frame
    let bx = f.minX + rx * f.width, by = f.minY + ry * f.height
    let p = NSPoint(x: bx + ansicht.buehne.frame.minX, y: ansicht.buehne.frame.minY + ansicht.buehne.bounds.height - by)
    // A real finger arrives with the pointer already over the window: the
    // hover pre-warm has run. Optional "kalt" = no hover (worst case).
    let kalt = profil.hasSuffix("kalt")
    if !kalt { Task { await aufnahme.vorwaermen(wid) } }
    let profil = profil.replacingOccurrences(of: "kalt", with: "")
    // (points per packet, finger phase, momentum phase)
    var pakete: [(Double, Int64, Int64)] = []
    func rausch(_ v: Double) -> Double { v * Double.random(in: 0.8...1.2) }
    switch profil {
    case "sehrlangsam":
        for i in 0..<60 { pakete.append((rausch(0.8), i == 0 ? 1 : 2, 0)) }
    case "langsam":
        for i in 0..<40 { pakete.append((rausch(3), i == 0 ? 1 : 2, 0)) }
        var v = 2.5; var i = 0
        while v > 0.4 { pakete.append((v, 0, i == 0 ? 1 : 2)); v *= 0.88; i += 1 }
    case "schnell":
        for v in [8.0, 25, 45, 55, 55, 50, 45] { pakete.append((rausch(v), pakete.isEmpty ? 1 : 2, 0)) }
        var v = 45.0; var i = 0
        while v > 0.5 { pakete.append((v, 0, i == 0 ? 1 : 2)); v *= 0.955; i += 1 }
    case "wisch":
        // Horizontal two-finger swipe (richtung +1 = fingers to the right).
        for v in [3.0, 8, 14, 18, 18, 16, 12, 8, 4] { pakete.append((rausch(v), pakete.isEmpty ? 1 : 2, 0)) }
    default: // normal
        for v in [2.0, 5, 8, 11, 12, 12, 12, 12, 11, 12, 12, 10, 9] { pakete.append((rausch(v), pakete.isEmpty ? 1 : 2, 0)) }
        var v = 10.0; var i = 0
        while v > 0.5 { pakete.append((v, 0, i == 0 ? 1 : 2)); v *= 0.94; i += 1 }
    }
    // Finger up between gesture and momentum; momentum end at the tail.
    if let k = pakete.firstIndex(where: { $0.2 != 0 }) { pakete.insert((0, 4, 0), at: k) } else { pakete.append((0, 4, 0)) }
    if pakete.last?.2 != 0 { pakete.append((0, 0, 3)) }
    var rest = 0.0, t = kalt ? 0.0 : 0.3, summe = 0
    for (v, ph, mo) in pakete {
        // Integer point deltas like the device delivers (fraction carried).
        let quer = profil == "wisch"
        rest += v * (quer ? (richtung > 0 ? 1 : -1) : (richtung > 0 ? -1 : 1))
        let d = Int(rest.rounded(.towardZero)); rest -= Double(d); summe += d
        let zeit = t
        DispatchQueue.main.asyncAfter(deadline: .now() + zeit) {
            guard let ce = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 2,
                                   wheel1: quer ? 0 : Int32(d), wheel2: quer ? Int32(d) : 0, wheel3: 0) else { return }
            ce.setIntegerValueField(.scrollWheelEventIsContinuous, value: 1)
            ce.setIntegerValueField(quer ? .scrollWheelEventPointDeltaAxis2 : .scrollWheelEventPointDeltaAxis1, value: Int64(d))
            ce.setDoubleValueField(quer ? .scrollWheelEventFixedPtDeltaAxis2 : .scrollWheelEventFixedPtDeltaAxis1, value: Double(d))
            ce.setIntegerValueField(.scrollWheelEventScrollPhase, value: ph)
            ce.setIntegerValueField(.scrollWheelEventMomentumPhase, value: mo)
            if let ne = NSEvent(cgEvent: ce) { ansicht.rollen(p, ne) }
        }
        t += Double.random(in: 0.0145...0.0190)
    }
    DispatchQueue.main.asyncAfter(deadline: .now() + t + 0.05) {
        sendeText("TESTRAD \(wid) fertig pakete=\(pakete.count) punkte=\(summe) dauer_ms=\(Int(t * 1000)) ziel=\(ansicht.fensterAn(p) ?? 0)")
    }
}

final class Fenster: NSObject, SCStreamOutput, SCStreamDelegate {
    let id: UInt32
    var farbeZeit = Date.distantPast
    var stream: SCStream?
    var rahmen: CGRect = .zero
    var totGemeldet = false
    var zuletztGeprueft = Date.distantPast
    var letztes: CMSampleBuffer?     // haelt die gezeigte IOSurface am Leben
    var standbild: CGImage?          // fallback fuer statische/off-Space Quellen ohne ersten Stream-Frame
    var cfg: SCStreamConfiguration?
    var schnell = true
    var hatBild = false
    // Bei einem Workspace-Retarget bleibt die alte, vollstaendige
    // Komposition sichtbar. Neue Streams sammeln ihr erstes Bild unsichtbar
    // und werden erst gemeinsam freigegeben.
    var veroeffentlicht = false
    var stilleBilder = 0
    var letzteSumme: UInt64 = 0
    /// Black-frame gate. Some apps stop painting while inactive and the
    /// WindowServer backing turns solid black (measured: ChatGPT/Codex
    /// 26.901 - black on the user's own display too, brightness returns only
    /// while the app is active). Once a window has shown real content, an
    /// all-black frame is never published over it; the last valid frame
    /// stays, and Noki is told (`SCHWARZ id 1/0`).
    var hatInhalt = false
    var schwarz = false
    var schwarzGemeldet = false
    /// Wie viele Bilder seit der letzten Inhaltsprobe vergangen sind.
    var seitProbe = 0
    /// Freshness is measured at all three boundaries: source callback,
    /// accepted/new content, and the main-thread compositor publication.
    /// The lock is necessary because SCStream calls on a per-window queue
    /// while diagnostics and recovery run elsewhere.
    private let frischSperre = NSLock()
    private var quellZeit = Date()
    private var neuZeit = Date.distantPast
    private var publizierZeit = Date.distantPast
    private var erwartetBis = Date.distantPast
    private var quellGeneration: UInt64 = 0
    private var compositorGeneration: UInt64 = 0
    private var publiziertGeneration: UInt64 = 0
    private let publishSperre = NSLock()
    private var publishPending = false
    private var pendingSample: CMSampleBuffer?
    private var pendingGeneration: UInt64 = 0
    var neustartLaeuft = false
    /// A `startCapture` for this window is in flight (background start after
    /// the swap): no second start from a newer generation.
    var startLaeuft = false
    init(id: UInt32) { self.id = id }

    func quelleGesehen() {
        frischSperre.lock()
        quellZeit = Date(); quellGeneration &+= 1
        frischSperre.unlock()
    }

    func neuerInhalt() {
        frischSperre.lock()
        neuZeit = Date()
        // If a moving source suddenly goes silent, keep enough time for the
        // watchdog to distinguish that from a genuinely static window.
        erwartetBis = Date().addingTimeInterval(8)
        frischSperre.unlock()
    }

    func aenderungErwarten() {
        frischSperre.lock()
        erwartetBis = max(erwartetBis, Date().addingTimeInterval(8))
        frischSperre.unlock()
        // Bedienung (Klick, Rad, Tippen): sofort voller Takt, und fuer eine
        // kurze Zeit nicht wieder drosseln - die Antwort des Programms soll
        // ohne 0,5-s-Probenraster sichtbar werden.
        lebhaftBis = Date().addingTimeInterval(1.5)
        stilleBilder = 0
        if stream == nil {
            Task { await aufnahme.quelleNeuStarten(id, grund: "input_on_nil_stream") }
        } else {
            takt(schnell: true)
        }
    }
    var lebhaftBis = Date.distantPast
    /// A scroll gesture is running on THIS window: its stream runs at 60/s
    /// for the gesture (+ momentum) so every page position Chrome renders
    /// reaches the Miniatur. Only this window - never all streams.
    var rollBis = Date.distantPast
    var bereitBis = Date.distantPast
    func vorwaermen() {
        bereitBis = Date().addingTimeInterval(2.5)
        lebhaftBis = max(lebhaftBis, bereitBis)
        stilleBilder = 0
        if !schnell || aktuellerTakt != 60 { takt(schnell: true) }
    }
    /// Scroll input arrived but no new content frame followed: the app did
    /// not commit its scroll on the hidden Space (measured: GoodNotes
    /// Marketplace - model scrolls, pixels frozen). Noki decides how to wake it.
    var starrPruefung = Date.distantPast
    var inhaltGeaendert = Date.distantPast
    func starrPruefen() {
        guard Date().timeIntervalSince(starrPruefung) > 0.4 else { return }
        starrPruefung = Date()
        let seit = Date()
        DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 0.3) { [self] in
            // Content-based: SCK marks every frame "complete" and unprobed
            // frames count as new, so only the sampled hash tells.
            frischSperre.lock(); let neu = inhaltGeaendert; frischSperre.unlock()
            if neu < seit { sendeText("STARR \(id)") }
        }
    }
    func rollen() {
        starrPruefen()
        let neu = Date() >= rollBis
        rollBis = Date().addingTimeInterval(0.45)
        lebhaftBis = max(lebhaftBis, Date().addingTimeInterval(1.5))
        erwartetBisVerlaengern()
        stilleBilder = 0
        if stream == nil {
            Task { await aufnahme.quelleNeuStarten(id, grund: "roll_on_nil_stream") }
        } else if neu || !schnell || aktuellerTakt != 60 {
            takt(schnell: true)
        }
    }
    func erwartetBisVerlaengern() {
        frischSperre.lock()
        erwartetBis = max(erwartetBis, Date().addingTimeInterval(8))
        frischSperre.unlock()
    }
    /// Vollstaendig von anderen Noki-Fenstern verdeckt (Kompositionsstapel):
    /// kein Veroeffentlichen, kein Zusammensetzen, Spartakt. Das neueste
    /// Bild wird trotzdem gehalten und beim Freilegen sofort gezeigt.
    var verdeckt = false
    func verdecktSetzen(_ v: Bool) {
        guard v != verdeckt else { return }
        verdeckt = v
        if v {
            if schnell && Date() > lebhaftBis { takt(schnell: false) }
        } else {
            stilleBilder = 0
            if !schnell { takt(schnell: true) }
            if let sb = letztes, let px = CMSampleBufferGetImageBuffer(sb),
               let f = CVPixelBufferGetIOSurface(px)?.takeUnretainedValue() {
                zeigeFlaeche(f, sb)
            }
        }
    }

    func kompositionVorgemerkt() -> UInt64 {
        frischSperre.lock(); defer { frischSperre.unlock() }
        compositorGeneration &+= 1
        return compositorGeneration
    }

    func publiziert(_ generation: UInt64) {
        frischSperre.lock()
        publiziertGeneration = max(publiziertGeneration, generation)
        publizierZeit = Date()
        frischSperre.unlock()
    }

    func frische() -> (source: Date, fresh: Date, publish: Date, expected: Date,
                       sourceGen: UInt64, compositorGen: UInt64, publishedGen: UInt64) {
        frischSperre.lock(); defer { frischSperre.unlock() }
        return (quellZeit, neuZeit, publizierZeit, erwartetBis,
                quellGeneration, compositorGeneration, publiziertGeneration)
    }

    /// Temporal tiers (PART 16): only the window the user looks at or just
    /// touched needs 30/s. A window that keeps changing in the background
    /// (Chrome omnibox animation, a page ticker, background video) gets 15/s;
    /// a still window 2/s. Spatial resolution is never lowered here.
    var vorn = false
    var aktuellerTakt: Int32 = 30
    func vornSetzen(_ v: Bool) {
        guard v != vorn else { return }
        vorn = v
        if schnell { takt(schnell: true) }
    }
    func takt(schnell s: Bool) {
        schnell = s
        // 60/s while scrolling: on this 60 Hz display SCK does not deliver
        // more (120 was measured: publish interval stayed 16.6-17 ms).
        // Passive live content (clock, spinner, video, a page animating by
        // itself) runs at LIVE_TAKT: enough useful unique frames for a
        // live Miniatur. 15-30/s here kept the helper at 8-31 % CPU
        // (+ WindowServer capture + a CoreAnimation commit per frame) for
        // as long as anything on the target Desktop moved - the MacBook ran
        // hot. Input on the window (1.5 s) still gets 30/s, scrolling and
        // hover pre-warm 60/s, still content 2/s. Resolution is untouched.
        let normal: Int32 = !s ? 2 : ((Date() < rollBis || Date() < bereitBis) ? 60 : (Date() < lebhaftBis ? 30 : LIVE_TAKT))
        // Space swipe running: the Miniatur stands still anyway (screen-fixed,
        // last sharp frame kept). WindowServer must not render window
        // captures in the middle of its slide animation.
        let ziel: Int32 = gestenRuhe && normal < 60 ? 1 : normal
        guard ziel != aktuellerTakt, let c = cfg, let st = stream else { return }
        aktuellerTakt = ziel
        c.minimumFrameInterval = CMTime(value: 1, timescale: ziel)
        st.updateConfiguration(c) { _ in }
    }

    func stream(_ s: SCStream, didOutputSampleBuffer sb: CMSampleBuffer, of t: SCStreamOutputType) {
        guard t == .screen else { return }
        if gestenRuhe { gestenBilder &+= 1 }
        quelleGesehen()
        if id == messWid { messSperre.lock(); messQuelle.append(Date().timeIntervalSince1970 * 1000); messSperre.unlock() }
        // Status ZUERST: Rahmen ohne Bild ("suspended") haben keinen
        // Bildpuffer - wer vorher auf ihn prueft, sieht das Schliessen nie.
        if let att = CMSampleBufferGetSampleAttachmentsArray(sb, createIfNecessary: false)
            as? [[SCStreamFrameInfo: Any]],
           let roh = att.first?[.status] as? Int,
           let st = SCFrameStatus(rawValue: roh), st != .complete {
            // Ein geschlossenes Fenster beendet den Strom NICHT mit einem
            // Fehler - er meldet nur noch "suspended" (gemessen, macOS 26.6).
            // Der Rahmen kommt oft, BEVOR WindowServer das Fenster austraegt,
            // deshalb kurz danach noch nachsehen. Vom Ereignis ausgeloest.
            if (st == .suspended || st == .stopped) && !totGemeldet
                && Date().timeIntervalSince(zuletztGeprueft) > 1 {
                zuletztGeprueft = Date()
                pruefeTot(nach: [0, 0.3, 1.0, 2.5])
            }
            if st == .suspended || st == .stopped { return }
            // `.idle` means unchanged, not unusable.  A newly started
            // off-Space stream for a static window often begins idle with a
            // perfectly valid IOSurface.  Dropping that first surface meant
            // `hatBild` could never become true, so every atomic retarget to
            // a Desktop containing a static window failed forever.  Once a
            // frame is retained, later idle samples remain cheap no-ops.
            if st != .idle || hatBild || CMSampleBufferGetImageBuffer(sb) == nil { return }
        }
        guard CMSampleBufferIsValid(sb), let px = CMSampleBufferGetImageBuffer(sb),
              let flaeche = CVPixelBufferGetIOSurface(px)?.takeUnretainedValue() else { return }
        // ANPASSUNG an die Bewegung. Gemessen: fuer Fenster auf einem
        // inaktiven Space meldet ScreenCaptureKit JEDES Bild als "complete"
        // mit dem ganzen Fenster als geaendertem Bereich - auch ein stilles
        // TextEdit 30/s. Entschieden wird deshalb am Inhalt selbst: eine
        // duenne Stichprobe (jede 8. Zeile, jedes 97. Byte). Unveraendert ->
        // kein neues Bild; nach einer Sekunde Stillstand nur noch 2
        // Abfragen/s. Die erste Aenderung hebt sofort wieder auf 30/s.
        // WANN ueberhaupt geprueft wird.
        //
        // Die Probe unten sperrt den Bildpuffer und zwingt die Flaeche dafuer
        // durch den Hauptspeicher. Bei einem stillen Fenster ist das billig
        // und spart viel; bei LAUFENDEM VIDEO kostet es jedes einzelne Bild
        // eine Synchronisation mit der Grafik - und genau daran ist die
        // vorher fluessige YouTube-Vorschau haengen geblieben.
        //
        // Waehrend Bewegung wird deshalb nur noch jedes achte Bild geprueft.
        // Das reicht, um den Stillstand binnen eines Viertelsekunde zu
        // bemerken, kostet aber im Bewegtbild fast nichts. Steht das Bild
        // bereits still, wird wie bisher jedes Mal geprueft - dort liefert
        // die Quelle ohnehin nur zwei Bilder je Sekunde.
        seitProbe += 1
        // At the passive live rate every 2nd frame is probed, so a source
        // that goes still steps down to 2/s as fast as before (~1-2 s).
        let pruefen = !schnell || seitProbe >= (aktuellerTakt > LIVE_TAKT ? 6 : 2)
        if verdeckt {
            // Verdeckt: nur das neueste Bild merken - nichts zusammensetzen.
            letztes = sb
            if schnell && Date() > lebhaftBis { takt(schnell: false) }
            return
        }
        if !pruefen && schwarz { return }
        if !pruefen {
            // Gemessen 2026-09-25: hier stand frueher `stilleBilder = 0` -
            // jedes ungepruefte Bild setzte den Stillstandszaehler zurueck,
            // kein Fenster fiel je auf den langsamen Takt (13 Fenster je
            // ~26 Bilder/s, obwohl sich 0 Pixel aenderten). Der Zaehler
            // gehoert allein den Proben.
            neuerInhalt()
            zeigeFlaeche(flaeche, sb)
            return
        }
        seitProbe = 0
        CVPixelBufferLockBaseAddress(px, .readOnly)
        var summe: UInt64 = 0
        var proben = 0, hell = 0
        if let basis = CVPixelBufferGetBaseAddress(px) {
            let zeilen = CVPixelBufferGetBytesPerRow(px)
            let hoehe = CVPixelBufferGetHeight(px)
            // EVERY pixel of every 2nd row (8-byte words = 2 whole BGRA
            // pixels each). The old grid (every 8th row, every 97th byte =
            // ~48 px apart on Retina) missed a terminal counter's digit
            // or a typed character - the Miniatur then kept the old frame.
            let worte = zeilen / 8
            var y = 0
            while y < hoehe {
                let zeile = UnsafeRawPointer(basis + y * zeilen)
                var x = 0
                while x < worte {
                    let v = zeile.loadUnaligned(fromByteOffset: x * 8, as: UInt64.self)
                    summe = (summe ^ v) &* 0x100000001b3
                    summe = (summe << 29) | (summe >> 35)
                    if x % 24 == 0 { proben += 1; if (v & 0xff) > 24 || ((v >> 8) & 0xff) > 24 { hell += 1 } }
                    x += 1
                }
                y += 2
            }
        }
        CVPixelBufferUnlockBaseAddress(px, .readOnly)
        let schwarzJetzt = proben > 50 && hell * 400 < proben
        if schwarzJetzt && hatInhalt {
            if !schwarz { schwarz = true; sendeText("SCHWARZ \(id) 1") }
            // Still a still source: step down to 2/s like any unchanged window.
            stilleBilder += 1
            if stilleBilder >= 5 && schnell && Date() > lebhaftBis { takt(schnell: false) }
            return
        }
        if !schwarzJetzt {
            hatInhalt = true
            if schwarz { schwarz = false; sendeText("SCHWARZ \(id) 0") }
        } else if !hatInhalt && !schwarzGemeldet {
            // Black from the very first frame (e.g. ChatGPT inactive since
            // it was restored): report it too, so Noki knows this app only
            // paints while active. Nothing to keep yet - the frame shows.
            schwarzGemeldet = true
            sendeText("SCHWARZ \(id) 1")
        }
        if summe == letzteSumme && hatBild {
            stilleBilder += 1
            // Schnell: Probe jedes 6. Bild -> 5 gleiche Proben ~ 1 s Stillstand.
            if stilleBilder >= 5 && schnell && Date() > lebhaftBis { takt(schnell: false) }
            return
        }
        letzteSumme = summe
        frischSperre.lock(); inhaltGeaendert = Date(); frischSperre.unlock()
        stilleBilder = 0
        hatBild = true
        neuerInhalt()
        // Also steps a background window down from 30 to 15 once the
        // interaction grace period is over.
        takt(schnell: true)
        zeigeFlaeche(flaeche, sb)
    }

    /// Ein Zeigerwechsel auf der Ebene dieses Fensters - kein Kodieren,
    /// kein Kopieren, kein Doppelbild.
    func zeigeFlaeche(_ flaeche: IOSurface, _ sb: CMSampleBuffer) {
        _ = flaeche // validation happened before enqueue; publication uses the newest retained sample
        letztes = sb
        standbild = nil
        guard veroeffentlicht else { return }
        let generation = kompositionVorgemerkt()
        // Coalesce on the source queue: at most one main-thread block per
        // window can be pending. If the main thread is briefly busy, old
        // video frames are replaced here instead of forming a stale FIFO.
        publishSperre.lock()
        pendingSample = sb
        pendingGeneration = generation
        if publishPending {
            publishSperre.unlock()
            return
        }
        publishPending = true
        publishSperre.unlock()
        DispatchQueue.main.async { [self] in
            publishSperre.lock()
            let aktuell = pendingSample
            let aktuellGeneration = pendingGeneration
            pendingSample = nil
            publishPending = false
            publishSperre.unlock()
            guard let aktuell,
                  let px = CMSampleBufferGetImageBuffer(aktuell),
                  let neuesteFlaeche = CVPixelBufferGetIOSurface(px)?.takeUnretainedValue() else { return }
            let l = ansicht.ebene(id)
            // Nur der ECHTE Inhalt: passt die Pufferrgroesse (noch) nicht zur
            // Fenstergroesse, legt ScreenCaptureKit das Fenster seitenrichtig
            // und mittig hinein (Raender). Ohne Zuschnitt wurde der ganze
            // Puffer samt Rand auf den Fensterrahmen gezogen: unscharf, und
            // sichtbare Tabs lagen woanders als die Trefferabbildung.
            let bw = CGFloat(CVPixelBufferGetWidth(px)), bh = CGFloat(CVPixelBufferGetHeight(px))
            var ausschnitt = CGRect(x: 0, y: 0, width: 1, height: 1)
            if let att = (CMSampleBufferGetSampleAttachmentsArray(aktuell, createIfNecessary: false)
                            as? [[SCStreamFrameInfo: Any]])?.first,
               let d = att[.contentRect] as? NSDictionary,
               let cr = CGRect(dictionaryRepresentation: d as CFDictionary), bw > 0, bh > 0 {
                let f = (att[.scaleFactor] as? CGFloat) ?? (att[.scaleFactor] as? Double).map { CGFloat($0) } ?? 1
                let px = CGRect(x: cr.minX * f, y: cr.minY * f, width: cr.width * f, height: cr.height * f)
                let n = CGRect(x: px.minX / bw, y: px.minY / bh, width: px.width / bw, height: px.height / bh)
                if n.width > 0.2, n.height > 0.2, n.maxX <= 1.01, n.maxY <= 1.01 { ausschnitt = n }
                geometrieSpur(id, puffer: CGSize(width: bw, height: bh), inhalt: px)
            }
            CATransaction.begin(); CATransaction.setDisableActions(true)
            l.contents = neuesteFlaeche
            if l.contentsRect != ausschnitt { l.contentsRect = ausschnitt }
            l.isHidden = false
            CATransaction.commit()
            publiziert(aktuellGeneration)
            bilderZaehler[id, default: 0] += 1
            if id == messWid { rollMessProbe(px, ausschnitt) }
            if Date().timeIntervalSince(farbeZeit) > 0.5 {
                farbeZeit = Date()
                ansicht.flickenFarbeMessen(id, px)
            }
        }
    }
    func pruefeTot(nach stufen: [Double]) {
        guard let erste = stufen.first else { return }
        DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + erste) { [self] in
            if totGemeldet { return }
            if fensterLebt(id) { pruefeTot(nach: Array(stufen.dropFirst())); return }
            totGemeldet = true
            sendeText("DEAD \(id)")
            Task { await aufnahme.vergessen(id) }
        }
    }
    func stream(_ s: SCStream, didStopWithError e: Error) {
        log("stream didStopWithError windowID=\(id) error=\(e)")
        sendeText("STREAM_ERROR \(id)")
        stream = nil
        Task {
            try? await Task.sleep(nanoseconds: 150_000_000)
            await aufnahme.quelleNeuStarten(id, grund: "did_stop_with_error")
        }
    }
}

/// Aufloesung eines Fensterbilds: genau so viele Pixel, wie die Miniatur an
/// dieser Stelle zeigt (Retina eingerechnet) - GROSS bekommt mehr, KOMPAKT
/// verschwendet nichts.
/// Pixel je virtuellem Punkt fuer die Aufnahme - ADAPTIV nach der
/// sichtbaren Ansicht (Miniatur / gross / Vollansicht), mit 1,25-facher
/// Ueberabtastung fuer eine saubere Verkleinerung, nie mehr als die
/// Vollansicht braucht. Gemessen: immer Vollansicht-Aufloesung fuer alle
/// Fenster (7 Fenster, ~15 MPixel x Warteschlange) liess WindowServer die
/// Fensterebenen der Miniatur ganz verwerfen - nur noch Hintergrund sichtbar.
/// Beim Oeffnen der Vollansicht wird neu vermessen (scharf in voller Groesse).
/// Capture oversampling of the Miniatur (integer, see `aufnahmeSkala`).
/// NOKI_MINI_OVERSAMPLE only exists for A/B measurements.
let UEBERABTASTUNG: CGFloat = {
    if let v = ProcessInfo.processInfo.environment["NOKI_MINI_OVERSAMPLE"], let d = Double(v), d >= 1, d <= 3 { return CGFloat(d) }
    return 2.0
}()

@MainActor func aufnahmeSkala() -> CGFloat {
    let backing = NSScreen.main?.backingScaleFactor ?? 2
    let vis = NSScreen.main?.visibleFrame ?? CGRect(x: 0, y: 0, width: 1470, height: 900)
    let w = max(1, schreibtisch.width), h = max(1, schreibtisch.height)
    let voll = min(1, (vis.width - 16) / w, (vis.height - 16 - FUSS) / h)
    let ansichtBreite = vollAn ? vollRahmen().width : (ansicht.istGross ? grossRahmen(kompaktRahmen).width : max(sollRahmen.width, 260))
    // Exactly the destination's physical pixels (was 1.25x). Oversampling
    // meant TWO resamplings - ScreenCaptureKit's downscale, then a bilinear
    // CoreAnimation minify (trilinear has no mip levels on an IOSurface) -
    // which visibly softened small text. At 1.0x SCK does the one
    // high-quality downscale and the layer maps 1:1 onto pixel-aligned
    // frames (`ordnen`).
    let jetzt = ansichtBreite / w
    // 2.0x oversample (integer!), capped at the window's native pixels.
    // Measured: at 1.0x the single ~4:1 ScreenCaptureKit downscale is
    // visibly softer than an ideal Lanczos downscale of the same window.
    // With 2.0x, SCK scales ~2:1 and the pixel-aligned layer (`ordnen`)
    // does an exact 2:1 box minify - two clean integer steps, crisp text.
    return min(voll, UEBERABTASTUNG * max(jetzt, 0.2)) * backing
}

@MainActor func zielGroesse(_ r: CGRect) -> (Int, Int) {
    let skala = aufnahmeSkala()
    return (max(16, Int((r.width * skala).rounded())), max(16, Int((r.height * skala).rounded())))
}

actor Aufnahme {
    var offen: [UInt32: Fenster] = [:]
    var fps = 30
    var pausiert = false
    var generation: UInt64 = 0

    var wiederbelebungen: Set<UInt32> = []

    func erwarteAenderung(_ id: UInt32) {
        if let f = offen[id] {
            f.aenderungErwarten()
            // Clicks and keys can change an off-Space Electron surface just
            // as scrolling can. Detect a renderer that accepted real input
            // but did not commit pixels; Rust decides whether to wake it.
            f.starrPruefen()
            if f.stream == nil {
                Task { await quelleNeuStarten(id, grund: "input_on_nil_stream") }
            }
        } else {
            Task { await quelleNeuStarten(id, grund: "input_missing_stream") }
        }
    }
    func rollt(_ id: UInt32) {
        if let f = offen[id] {
            f.rollen()
            if f.stream == nil {
                Task { await quelleNeuStarten(id, grund: "roll_on_nil_stream") }
            }
        } else {
            Task { await quelleNeuStarten(id, grund: "roll_missing_stream") }
        }
    }
    func vorwaermen(_ id: UInt32) {
        offen[id]?.vorwaermen()
    }

    func frischeZeilen(fps: [UInt32: Int], interaction: String, workerBusy: Bool) -> [String] {
        let jetzt = Date()
        return offen.sorted { $0.key < $1.key }.map { id, f in
            let s = f.frische()
            let alter: (Date) -> String = { d in
                d == .distantPast ? "-1" : String(format: "%.3f", jetzt.timeIntervalSince(d))
            }
            return "FRESH virtualDisplayID=\(schreibtischAnzeige) windowID=\(id) captureSourceID=\(id)"
                + " lastSourceFrameAge=\(alter(s.source)) lastNewFrameAge=\(alter(s.fresh))"
                + " compositorGeneration=\(s.compositorGen) publishedGeneration=\(s.publishedGen)"
                + " lastPublishAge=\(alter(s.publish)) previewFPS=\(fps[id, default: 0])"
                + " interactionState=\(interaction) workerBusy=\(workerBusy)"
        }
    }

    /// Restart only a source that was demonstrably live and then stopped.
    /// Its current CALayer stays untouched until the replacement publishes
    /// a real frame, so recovery never flashes wallpaper or an empty stage.
    func frischePruefen() async {
        let jetzt = Date()
        let ortAktuell = await MainActor.run { ort }
        let sichtbarAktuell = await MainActor.run { sichtbar }
        if ortAktuell != "noki" && sichtbarAktuell && pausiert {
            pausiert = false
        }
        guard !pausiert else { return }

        // A: Check visible layers: any layer missing from offen or with nil stream
        let sichtbareIds = await MainActor.run {
            Array(ansicht.fensterEbenen.keys.filter { ansicht.fensterEbenen[$0]?.isHidden == false })
        }
        for id in sichtbareIds {
            if let f = offen[id] {
                if f.stream == nil && !wiederbelebungen.contains(id) {
                    log("frischePruefen: visible window \(id) has nil stream, restarting")
                    await quelleNeuStarten(id, grund: "nil_stream")
                }
            } else if !wiederbelebungen.contains(id) {
                log("frischePruefen: visible window \(id) missing from offen, restarting")
                await quelleNeuStarten(id, grund: "missing_from_offen")
            }
        }

        // B: Check active streams for stalls
        for (id, f) in offen where f.veroeffentlicht && !wiederbelebungen.contains(id) {
            let s = f.frische()
            let sourceAge = jetzt.timeIntervalSince(s.source)
            let publishAge = s.publish == .distantPast ? -1 : jetzt.timeIntervalSince(s.publish)
            let interacting = jetzt < f.rollBis || jetzt < f.bereitBis || f.lebhaftBis > jetzt
            if interacting && sourceAge > 0.8 {
                await quelleNeuStarten(id, grund: String(format: "interaction_stall_%.1fs", sourceAge))
            } else if s.expected > jetzt && sourceAge > 2.0 {
                await quelleNeuStarten(id, grund: String(format: "source_stall_%.1fs", sourceAge))
            } else if s.compositorGen > s.publishedGen + 30 && publishAge > 2.0 {
                await quelleNeuStarten(id, grund: "publication_stall")
            }
        }
    }

    func quelleNeuStarten(_ id: UInt32, grund: String) async {
        guard !pausiert, !wiederbelebungen.contains(id) else { return }
        // Its stream is already being started (show-first swap, parallel
        // background start): a second, serial restart here replaced every
        // source again (measured: 10 x "input_on_nil_stream" right after
        // each switch, each ~90 ms, the parallel streams then discarded).
        if offen[id]?.startLaeuft == true { return }
        wiederbelebungen.insert(id)
        defer { wiederbelebungen.remove(id) }
        let alt = offen[id]
        do {
            let inhalt = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
            guard let w = inhalt.windows.first(where: { $0.windowID == id }), fensterLebt(id) else {
                sendeText("DEAD \(id)")
                await vergessen(id)
                return
            }
            let neu = Fenster(id: id)
            neu.veroeffentlicht = true
            neu.rahmen = w.frame
            neu.aenderungErwarten()
            let c = await konfig(w); neu.cfg = c
            let s = SCStream(filter: SCContentFilter(desktopIndependentWindow: w), configuration: c, delegate: neu)
            try s.addStreamOutput(neu, type: .screen,
                                  sampleHandlerQueue: DispatchQueue(label: "noki.schirm.\(id).recovery"))
            try await s.startCapture()
            neu.stream = s
            // Publish ownership only after the replacement stream started.
            // The old IOSurface remains the layer content in the meantime.
            offen[id] = neu
            try? await alt?.stream?.stopCapture()
            alt?.stream = nil
            log("freshness restart windowID=\(id) captureSourceID=\(id) reason=\(grund)")
        } catch {
            log("freshness restart failed windowID=\(id) reason=\(grund) error=\(error)")
        }
    }

    func konfig(_ w: SCWindow) async -> SCStreamConfiguration {
        let cfg = SCStreamConfiguration()
        let (pw, ph) = await zielGroesse(w.frame)
        cfg.width = pw
        cfg.height = ph
        // Resize must never crop: when the real window grows beyond the
        // configured size before the stream is re-measured, SCK must scale
        // the WHOLE window into the buffer (letterbox is removed again via
        // the frame's contentRect), never deliver only its top-left part.
        // (Measured macOS 26.6: also setting `preservesAspectRatio = true`
        // made every window stream deliver NO frame - cold start FAILED.)
        cfg.scalesToFit = true
        // Obergrenze, keine Taktung: ScreenCaptureKit liefert nur bei
        // Aenderung. Still = fast nichts; Video/Scrollen = bis 30 Bilder/s.
        cfg.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(fps))
        cfg.queueDepth = 3
        cfg.showsCursor = false
        cfg.pixelFormat = kCVPixelFormatType_32BGRA
        return cfg
    }

    /// Starts streams with at most `gleichzeitig` concurrent `startCapture`
    /// calls. A failing window never aborts the others (measured: a Mail
    /// window without stream blocked the whole atomic cold start).
    static func parallelStarten(_ auftraege: [(UInt32, SCWindow, Fenster, SCStreamConfiguration)],
                                gleichzeitig: Int,
                                gueltig: @escaping @Sendable (UInt32, Fenster) async -> Bool) async -> [(UInt32, Fenster, SCStream?)] {
        var ergebnis: [(UInt32, Fenster, SCStream?)] = []
        var i = 0
        await withTaskGroup(of: (UInt32, Fenster, SCStream?).self) { g in
            func naechster() {
                guard i < auftraege.count else { return }
                let (id, w, f, c) = auftraege[i]; i += 1
                g.addTask {
                    guard await gueltig(id, f) else { return (id, f, nil) }
                    let s = SCStream(filter: SCContentFilter(desktopIndependentWindow: w), configuration: c, delegate: f)
                    do {
                        try s.addStreamOutput(f, type: .screen, sampleHandlerQueue: DispatchQueue(label: "noki.schirm.\(id)"))
                        try await s.startCapture()
                        return (id, f, s)
                    } catch {
                        log("stream start failed \(id): \(error)")
                        sendeText("STARTFEHLER \(id)")
                        return (id, f, nil)
                    }
                }
            }
            for _ in 0..<gleichzeitig { naechster() }
            for await (id, f, s) in g {
                ergebnis.append((id, f, s))
                naechster()
            }
        }
        return ergebnis
    }

    /// Space swipe begins/ends: all streams to 1/s and back (one
    /// `updateConfiguration` per stream, no restart, no membership change).
    func gestenRuheSetzen(_ an: Bool) {
        guard an != gestenRuhe else { return }
        if an { gestenBilder = 0; gestenStart = Date() }
        else { log("geste ruhe aus: ms=\(Int(Date().timeIntervalSince(gestenStart) * 1000)) bilder=\(gestenBilder) streams=\(offen.values.filter { $0.stream != nil }.count)") }
        gestenRuhe = an
        for (_, f) in offen where f.stream != nil { f.takt(schnell: f.schnell) }
    }

    /// Is `f` still THE source of window `id` (not replaced/forgotten)?
    func istAktuell(_ id: UInt32, _ f: Fenster) -> Bool { offen[id] === f }

    /// Starts live streams for sources that are already registered (their
    /// picture may already be shown). A stream is only attached if its
    /// source is still current and has none - otherwise it is stopped.
    /// Returns the ids that got a stream.
    @discardableResult
    func streamsStarten(_ auftraege: [(UInt32, SCWindow, Fenster, SCStreamConfiguration)]) async -> [UInt32] {
        guard !auftraege.isEmpty else { return [] }
        let t0 = Date()
        let erg = await Aufnahme.parallelStarten(auftraege, gleichzeitig: 6) { [weak self] id, f in
            guard let self else { return false }
            return await self.istAktuell(id, f)
        }
        var mit: [UInt32] = []
        for (id, f, s) in erg {
            f.startLaeuft = false
            guard let s else { continue }
            if offen[id] === f && f.stream == nil {
                f.stream = s
                mit.append(id)
            } else {
                try? await s.stopCapture()
            }
        }
        log("live streams started n=\(mit.count)/\(auftraege.count) ms=\(Int(Date().timeIntervalSince(t0) * 1000))")
        return mit
    }

    /// Batch first snapshot: the WindowServer backing store of each window
    /// (last painted content, full resolution, also on hidden Spaces), taken
    /// in parallel - one batch instead of N stream start-ups.
    nonisolated static func standbilderParallel(_ ids: [UInt32]) -> [UInt32: CGImage] {
        guard !ids.isEmpty else { return [:] }
        var out: [UInt32: CGImage] = [:]
        let lock = NSLock()
        DispatchQueue.concurrentPerform(iterations: ids.count) { i in
            if let b = fensterStandbild(ids[i], nominal: true) { lock.lock(); out[ids[i]] = b; lock.unlock() }
        }
        return out
    }

    func setze(_ ids: [UInt32], erzwingen: Bool = false) async {
        guard !pausiert else { return }
        let tAnfrage = Date()
        generation &+= 1
        let meineGen = generation
        let ziel = Set(ids)
        let alt = Set(offen.compactMap { $0.value.veroeffentlicht ? $0.key : nil })
        let retarget = erzwingen || ziel != alt
        var neuGestartet: [UInt32] = []
        var startAuftraege: [(UInt32, SCWindow, Fenster, SCStreamConfiguration)] = []
        var ohneBild: [UInt32] = []
        var sofortZeigen = false
        // Superseded before the background start: free the start claim, so a
        // newer generation can start these sources.
        defer { for j in startAuftraege where j.2.stream == nil { j.2.startLaeuft = false } }
        var kandidateOrte: [UInt32: CGRect] = [:]
        if retarget { await MainActor.run { zielWechselLaeuft = true } }
        // However this generation ends (swap, rejection, superseded): the
        // label hold-back ends with it - but only if no newer switch runs.
        defer { if retarget { Task { if await aufnahme.generation == meineGen { await MainActor.run { zielWechselLaeuft = false } } } } }
        do {
            // onScreenWindowsOnly:false -> auch Fenster auf inaktiven Spaces
            let inhalt = try await SCShareableContent.excludingDesktopWindows(
                false, onScreenWindowsOnly: false)
            // Latest request wins BEFORE any stream starts: this actor is
            // re-entered at every await, and at cold start 10 generations
            // each started their own streams concurrently (6.8 s to swap).
            if self.generation != meineGen {
                log("setze generation \(meineGen) verworfen vor Start (neu=\(self.generation))")
                return
            }
            let z = stapel()
            for id in ids {
                if self.generation != meineGen { return }
                guard let w = inhalt.windows.first(where: { $0.windowID == id }) else {
                    sendeText(fensterLebt(id) ? "GONE \(id)" : "DEAD \(id)"); continue
                }
                let app = (w.owningApplication?.applicationName ?? "?")
                    .replacingOccurrences(of: " ", with: "_")
                // Platz und Stapel bei JEDEM Abgleich frisch: der Nutzer kann
                // dort Fenster verschoben oder nach vorn geholt haben. Beim
                // Space-Swipe liefert SCK jedoch kurz die WindowServer-
                // Transformlage (z. B. x=-1861) statt der finalen Desktop-
                // Lage. Solche nahezu vollstaendig ausserhalb des Ziel-
                // Desktops liegenden Zwischenrahmen duerfen weder Rendering
                // noch Hit-Test verschieben: last-known-valid bleibt stehen.
                let rohRahmen = w.frame
                let schnitt = rohRahmen.intersection(schreibtisch)
                let anteil = (rohRahmen.width > 0 && rohRahmen.height > 0 && !schnitt.isNull)
                    ? (schnitt.width * schnitt.height) / (rohRahmen.width * rohRahmen.height) : 0
                let altRahmen = await MainActor.run { ansicht.orte[id] }
                let transient = anteil < 0.25 && altRahmen != nil
                let rahmen = transient ? altRahmen! : rohRahmen
                if transient {
                    sendeText("GEOMETRY_LOCK \(id) transient=\(Int(rohRahmen.minX)),\(Int(rohRahmen.minY)) kept=\(Int(rahmen.minX)),\(Int(rahmen.minY))")
                } else {
                    sendeText("META \(id) \(Int(rahmen.origin.x)) \(Int(rahmen.origin.y))"
                            + " \(Int(rahmen.width)) \(Int(rahmen.height)) \(app)")
                }
                kandidateOrte[id] = rahmen
                if !retarget {
                    await MainActor.run {
                        ansicht.orte[id] = rahmen
                        // Dieselbe Regel wie beim Nachziehen: unbekannt heisst
                        // "nicht sichtbar, also nichts Neues zu sagen".
                        if let neu = z[id] { ansicht.ebene(id).zPosition = CGFloat(neu) }
                        else { _ = ansicht.ebene(id) }
                        ansicht.ordnen()
                    }
                }
                if let f = offen[id] {
                    if f.stream == nil {
                        // Started with the others (in parallel, after the
                        // swap) - never inline and serial here.
                        if !f.startLaeuft {
                            let c = await konfig(w); f.cfg = c; f.schnell = true
                            f.startLaeuft = true
                            startAuftraege.append((id, w, f, c))
                        }
                        if !f.hatBild { ohneBild.append(id) }
                    } else if f.rahmen.size != rahmen.size {
                        f.rahmen = rahmen
                        let c = await konfig(w); f.cfg = c; f.schnell = true
                        try? await f.stream?.updateConfiguration(c)
                    }
                    continue
                }
                let f = Fenster(id: id)
                // A brand-new source must also participate in the targeted
                // freshness watchdog. Otherwise a stream which stalls before
                // its first useful frame could retain an empty/old preview
                // indefinitely without ever becoming eligible for recovery.
                f.veroeffentlicht = !retarget
                f.rahmen = rahmen
                // Registered and claimed BEFORE any await: `aenderungErwarten`
                // with no stream spawns a restart task, and during the
                // `konfig` await that task used to find no source and build
                // its own - serially, for every window of every switch.
                f.startLaeuft = true
                offen[id] = f
                f.aenderungErwarten()
                let c = await konfig(w); f.cfg = c
                // Registered now (no stream yet): the swap shows its snapshot
                // or cached picture; the live stream attaches afterwards.
                startAuftraege.append((id, w, f, c))
                ohneBild.append(id)
            }
            if self.generation != meineGen { return }

            // SHOW FIRST, LIVE SECOND. The visible switch no longer waits for
            // N stream start-ups (~200 ms each; 11 windows = 2.5 s serial,
            // ~0.5 s in parallel): every window needs only ONE picture -
            // its last valid cached frame, or a batch snapshot of its
            // WindowServer backing store - and the complete generation is
            // swapped at once. Streams then start in the background and
            // replace the pictures as their first frames arrive.
            // A layer that still SHOWS a valid picture of this window (e.g.
            // after `neuStarten`, which only drops sources) counts like a
            // cached one - no new snapshot needed.
            let zwischenVorab = await MainActor.run {
                Set(ids.filter { bildCache[$0] != nil || ansicht.fensterEbenen[$0]?.contents != nil })
            }
            let bedarf = ohneBild.filter { !zwischenVorab.contains($0) }
            let tSnap = Date()
            // Off the actor: while WindowServer renders the snapshots the
            // actor must stay free (PULSE is answered by it; a blocked actor
            // for ~1 s at startup made Noki send `neustart`, which cleared all
            // sources and started the next snapshot batch - a loop).
            let bilder = await withCheckedContinuation { (k: CheckedContinuation<[UInt32: CGImage], Never>) in
                DispatchQueue.global(qos: .userInitiated).async { k.resume(returning: Aufnahme.standbilderParallel(bedarf)) }
            }
            for (id, bild) in bilder { if let f = offen[id] { f.standbild = bild; f.hatBild = true } }
            if !bedarf.isEmpty {
                log("batch snapshot n=\(bilder.count)/\(bedarf.count) ms=\(Int(Date().timeIntervalSince(tSnap) * 1000))")
            }
            if self.generation != meineGen { return }
            let hatBildOderCache: (UInt32) -> Bool = { id in
                guard let f = self.offen[id] else { return false }
                return (f.hatBild && (f.letztes != nil || f.standbild != nil)) || zwischenVorab.contains(id)
            }
            sofortZeigen = retarget && ids.allSatisfy(hatBildOderCache)
            if !sofortZeigen {
                // A window without any picture (snapshot failed, never seen):
                // it needs its stream BEFORE the swap - the old path.
                neuGestartet = await streamsStarten(startAuftraege)
                startAuftraege = []
                if self.generation != meineGen { return }
            }

            if retarget {
                // Kein altes Bild, kein Wallpaper-only und kein fremder
                // Zwischenstand: erst wenn JEDES Ziel-Fenster ein echtes
                // erstes Bild besitzt, wird in EINER CATransaction ersetzt.
                //
                // Ein Fenster, das schon einmal gezeigt wurde, hat sein
                // letztes gueltiges Bild im Zwischenspeicher: damit ist die
                // Komposition sofort vollstaendig. Frische Bilder bekommen
                // noch einen kurzen Moment (0,35 s), dann wird getauscht -
                // die frischen folgen als normale Aktualisierung.
                let t0 = Date()
                let streamFrist = t0.addingTimeInterval(1.2)
                let frisch: (UInt32) -> Bool = { id in
                    guard let f = self.offen[id] else { return false }
                    return f.hatBild && (f.letztes != nil || f.standbild != nil)
                }
                let zwischen = await MainActor.run { Set(ids.filter { bildCache[$0] != nil }) }
                // Freshness order: live stream frame > WindowServer backing
                // store (the window's LAST PAINTED content, full resolution,
                // also for hidden-Space windows) > old cache.
                // Show-first generation: every window already has a picture
                // (snapshot or last valid) - no waiting at all.
                while !sofortZeigen && Date() < streamFrist && !ids.allSatisfy(frisch) {
                    if self.generation != meineGen {
                        log("setze generation \(meineGen) abgebrochen (neu=\(self.generation))")
                        return
                    }
                    if Date().timeIntervalSince(t0) > 0.15 {
                        for id in ids where !frisch(id) {
                            guard let f = offen[id], let bild = fensterStandbild(id) else { continue }
                            f.standbild = bild; f.hatBild = true
                            log("backing store \(id) \(bild.width)x\(bild.height)")
                        }
                        if ids.allSatisfy({ frisch($0) || zwischen.contains($0) }) { break }
                    }
                    try? await Task.sleep(nanoseconds: 20_000_000)
                }
                if self.generation != meineGen { return }
                for id in ids {
                    guard let f = offen[id], !f.hatBild else { continue }
                    if let bild = fensterStandbild(id) {
                        f.standbild = bild
                        f.hatBild = true
                        log("standbild fallback \(id)")
                    }
                }
                let hatEbenen = await MainActor.run { !ansicht.fensterEbenen.isEmpty }
                if ids.isEmpty {
                    if erzwingen {
                        await MainActor.run {
                            CATransaction.begin(); CATransaction.setDisableActions(true)
                            for id in Array(ansicht.fensterEbenen.keys) {
                                bildMerken(id)
                                ansicht.entfernen(id)
                            }
                            ansicht.ordnen()
                            CATransaction.commit()
                            zielMetaAnwenden()
                            ersteKompositionFertig("retarget_leer")
                        }
                        for id in alt {
                            if let f = offen[id] { try? await f.stream?.stopCapture(); f.stream = nil }
                            offen[id] = nil
                        }
                        sendeText("READY 0")
                        return
                    } else if hatEbenen {
                        log("empty retarget inventory rejected; retaining last valid composition")
                        sendeText("ERR Retarget leer verworfen; altes Bild bleibt")
                        return
                    }
                }
                let bereit = sofortZeigen || ids.allSatisfy { frisch($0) || (offen[$0] != nil && zwischen.contains($0)) }
                if !bereit && !ids.isEmpty {
                    // A capture timeout is not evidence that the real window
                    // disappeared. Keep the complete last-valid composition,
                    // tear down only the unpublished candidate sources, and
                    // let the host's bounded inventory loop retry. Publishing
                    // the ready subset here caused the observed 5 -> 3 window
                    // SWAP and could leave the Miniatur looking like wallpaper.
                    for id in neuGestartet {
                        if let f = offen[id] { try? await f.stream?.stopCapture(); f.stream = nil }
                        offen[id] = nil
                    }
                    let fehlend = ids.filter { !frisch($0) && !(offen[$0] != nil && zwischen.contains($0)) }
                    log("retarget rejected missing=\(fehlend); last valid composition retained")
                    sendeText("ERR Retarget nicht vollstaendig missing=\(fehlend.count); altes Bild bleibt")
                    return
                }
                let freigeben: [(UInt32, Fenster)] = ids.compactMap { id in
                    offen[id].map { (id, $0) }
                }
                let festeOrte = kandidateOrte
                if self.generation != meineGen { return }
                await MainActor.run {
                    CATransaction.begin(); CATransaction.setDisableActions(true)
                    for id in Array(ansicht.fensterEbenen.keys) where !ziel.contains(id) {
                        bildMerken(id)
                        ansicht.entfernen(id)
                    }
                    for (id, f) in freigeben {
                        if let r = festeOrte[id] {
                            ansicht.orte[id] = r
                        }
                        let l = ansicht.ebene(id)
                        if let neu = z[id] { l.zPosition = CGFloat(neu) }
                        if let sb = f.letztes,
                           let px = CMSampleBufferGetImageBuffer(sb),
                           let flaeche = CVPixelBufferGetIOSurface(px)?.takeUnretainedValue() {
                            l.contents = flaeche
                        } else if let bild = f.standbild {
                            l.contents = bild
                        } else if l.contents == nil, let alt = bildCache[id] {
                            // Letztes gueltiges Bild; das frische folgt.
                            l.contents = alt
                        }
                        // Ohne jedes Bild bleibt die Ebene verborgen (nie eine
                        // leere Attrappe) und erscheint mit ihrem ersten Bild.
                        l.isHidden = l.contents == nil
                        f.veroeffentlicht = true
                        f.publiziert(f.kompositionVorgemerkt())
                    }
                    zielMetaAnwenden()
                    ansicht.ordnen()
                    CATransaction.commit()
                    // TEMPORARY audit: the first published frame must show
                    // every target window (fresh or last-valid), never a
                    // wallpaper-only or partial build-up.
                    let gezeigt = freigeben.filter { ansicht.fensterEbenen[$0.0]?.isHidden == false }.count
                    sendeText("SWAP ids=\(ids.count) shown=\(gezeigt) cached=\(zwischen.count) fresh=\(ids.filter(frisch).count) ms=\(Int(Date().timeIntervalSince(t0) * 1000)) request_to_swap_ms=\(Int(Date().timeIntervalSince(tAnfrage) * 1000)) show_first=\(sofortZeigen)")
                    hoverAbgleichen()
                    ersteKompositionFertig("retarget")
                }
                // Erst NACH dem atomaren visuellen Tausch alte Quellen
                // loesen. So kann kein Frame des Nutzer-Schreibtischs oder
                // ein Wallpaper-only-Zustand dazwischen sichtbar werden.
                for id in alt where !ziel.contains(id) {
                    if let f = offen[id] { try? await f.stream?.stopCapture(); f.stream = nil }
                    offen[id] = nil
                }
            }
            // LIVE SECOND: the picture is already on screen.
            if !startAuftraege.isEmpty { await streamsStarten(startAuftraege) }
            sendeText("READY \(offen.count)")
            if kompositionVorn != 0 { stapelNachziehen() }
        } catch {
            sendeText("ERR \(error)")
        }
    }

    /// Nach einer Bildschirm-Neuordnung liefern bestehende Stroeme still
    /// nichts mehr (gemessen: die virtuelle Anzeige wurde von WindowServer
    /// umgesetzt, die Miniatur blieb auf einem alten Stand stehen, waehrend
    /// eine frische Aufnahme desselben Fensters den echten Inhalt zeigte).
    /// Alle Stroeme werden neu begonnen; die alten Ebenen behalten ihr Bild,
    /// bis die neuen gemeinsam freigegeben werden - kein Leerbild dazwischen.
    func neuStarten() async {
        let veroeffentlicht = offen.compactMap { $0.value.veroeffentlicht ? $0.key : nil }
        let ids = hostZiel.map { z in veroeffentlicht.filter { z.contains($0) } } ?? veroeffentlicht
        guard !ids.isEmpty, !pausiert else { return }
        for versuch in 0..<4 {
            for (id, f) in offen { try? await f.stream?.stopCapture(); f.stream = nil; offen[id] = nil }
            log("neustart \(ids) versuch \(versuch)")
            await setze(ids.sorted(), erzwingen: true)
            // Nicht vollstaendig (z. B. ein Fenster lieferte noch kein Bild):
            // die alten Ebenen stehen noch - nach kurzer Pause erneut, statt
            // ohne jeden Strom zu bleiben.
            if ids.allSatisfy({ offen[$0]?.veroeffentlicht == true }) { return }
            try? await Task.sleep(nanoseconds: 1_000_000_000)
        }
    }

    /// Neue Miniaturgroesse: dieselben Stroeme, nur mit passender Aufloesung.
    func neuVermessen() async {
        // Ohne SCShareableContent (gemessen: 661 Fenster aufzaehlen + 13
        // XPC-Umkonfigurationen je Hover-Wechsel liessen die naechste
        // Hover-Animation bis 0,8 s spaet starten). Die Fenstergroessen kennt
        // die Ansicht bereits; umkonfiguriert wird nur bei echter Aenderung.
        for (id, f) in offen {
            guard let c = f.cfg, let r = await MainActor.run(body: { ansicht.orte[id] }),
                  r.width > 4, r.height > 4 else { continue }
            let (pw, ph) = await zielGroesse(r)
            if abs(pw - c.width) <= max(2, c.width / 50) && abs(ph - c.height) <= max(2, c.height / 50) { continue }
            c.width = pw; c.height = ph
            if !f.verdeckt {
                let t: Int32 = Date() < f.lebhaftBis ? 30 : LIVE_TAKT
                f.schnell = true; f.aktuellerTakt = t; c.minimumFrameInterval = CMTime(value: 1, timescale: t)
            }
            try? await f.stream?.updateConfiguration(c)
        }
    }

    /// Seconds since each window last delivered NEW content (not merely an
    /// unchanged sample).
    func bildAlter() -> [UInt32: Double] {
        var out: [UInt32: Double] = [:]
        for (id, f) in offen { out[id] = Date().timeIntervalSince(f.frische().fresh) }
        return out
    }

    /// Published frame generation per window (audit: frames actually shown).
    func bildGeneration() -> [UInt32: UInt64] {
        var out: [UInt32: UInt64] = [:]
        for (id, f) in offen { out[id] = f.frische().publishedGen }
        return out
    }

    /// Independent liveness sample for the host watchdog. `capture` means
    /// every published stream has supplied a source sample recently (or the
    /// composition is intentionally empty/paused); `generation` proves what
    /// the compositor has actually published, rather than merely queued.
    func puls() -> (streams: Int, capture: Bool, generation: UInt64) {
        if pausiert { return (offen.count, true, offen.values.map { $0.frische().publishedGen }.max() ?? 0) }
        let aktiv = offen.values.filter { $0.veroeffentlicht }
        let capture = aktiv.isEmpty || aktiv.allSatisfy {
            Date().timeIntervalSince($0.frische().source) < 4.0
        }
        return (aktiv.count, capture, aktiv.map { $0.frische().publishedGen }.max() ?? 0)
    }

    func verdeckungSetzen(_ v: [UInt32: Bool], vorn: UInt32?) {
        for (id, x) in v { offen[id]?.verdecktSetzen(x) }
        for (id, f) in offen { f.vornSetzen(id == vorn) }
    }

    func vergessen(_ id: UInt32) async {
        if let f = offen[id] { try? await f.stream?.stopCapture(); f.stream = nil; offen[id] = nil }
        await MainActor.run { ansicht.entfernen(id) }
    }

    func pause() async {
        pausiert = true
        for (_, f) in offen {
            try? await f.stream?.stopCapture()
            f.stream = nil
        }
        sendeText("PAUSED")
    }

    func entpausieren() {
        pausiert = false
    }

    /// The window set the HOST last asked for (`set`/start). The only
    /// authority for what belongs to the current target Desktop.
    var hostZiel: [UInt32]? = nil
    func hostZielSetzen(_ ids: [UInt32]) { hostZiel = ids }

    func weiter() async {
        pausiert = false
        // Revive EXACTLY the host's current set. The former union of all
        // open streams and layers re-added the previous target's windows
        // while their streams were still being stopped after a retarget
        // (measured 2026-09-29: btop of Desktop 1 re-appeared 270 ms after
        // the swap to Desktop 2 = VS Code only).
        let ebenen = await MainActor.run { Array(ansicht.fensterEbenen.keys) }
        let alle = hostZiel ?? Array(Set(offen.keys).union(ebenen))
        if !alle.isEmpty {
            let brauchtStream = alle.filter { offen[$0] == nil || offen[$0]?.stream == nil }
            if !brauchtStream.isEmpty {
                log("weiter: reviving streams for \(brauchtStream)")
                await setze(alle, erzwingen: true)
            }
        }
    }
}

/// Letztes gueltiges Bild je Fenster (IOSurface/CGImage), ueber einen
/// Zielwechsel hinaus: blaettert der Nutzer zu einem Schreibtisch zurueck,
/// steht dessen Komposition sofort vollstaendig da. Klein gehalten.
var bildCache: [UInt32: Any] = [:]
var bildCacheReihe: [UInt32] = []
func bildMerken(_ id: UInt32) {
    guard let c = ansicht.fensterEbenen[id]?.contents else { return }
    bildCache[id] = c
    bildCacheReihe.removeAll { $0 == id }
    bildCacheReihe.append(id)
    while bildCacheReihe.count > 24 { bildCache[bildCacheReihe.removeFirst()] = nil }
}

let aufnahme = Aufnahme()
var startIds: [UInt32] = []
var it = CommandLine.arguments.dropFirst().makeIterator()
while let a = it.next() {
    switch a {
    case "--windows": startIds = (it.next() ?? "").split(separator: ",").compactMap { UInt32($0) }
    default: _ = it.next()
    }
}

sendeSchreibtisch()
/// Stapel neu legen - sonst nichts.
///
/// Holt der Nutzer auf Nokis Schreibtisch ein Fenster nach vorn, aendert
/// sich WEDER Inhalt NOCH Platz, nur die Reihenfolge. Vorher wurde die erst
/// beim naechsten vollen Abgleich nachgezogen; bis dahin lag in der Miniatur
/// weiter YouTube ueber Claude, obwohl in Wirklichkeit Claude vorn stand.
/// Kein Strom wird angefasst, kein Bild neu geholt: nur zPosition.
/// Von Noki gewaehltes vorderes Fenster (Komposition), 0 = echte Stapelung.
var kompositionVorn: UInt32 = 0

func stapelNachziehen() {
    var z = stapel()
    if kompositionVorn != 0 {
        let hoechstes = z.filter { ansicht.fensterEbenen[$0.key] != nil }.map { $0.value }.max() ?? 0
        z[kompositionVorn] = hoechstes + 10
    }
    log("stapel \(z.filter { ansicht.fensterEbenen[$0.key] != nil })")
    DispatchQueue.main.async {
        CATransaction.begin(); CATransaction.setDisableActions(true)
        var bekannt = false
        for (id, l) in ansicht.fensterEbenen {
            // Kennt die Liste das Fenster nicht, steht der Nutzer gerade
            // woanders - dann bleibt die zuletzt gueltige Lage stehen,
            // statt alles auf 0 zu werfen.
            if let neu = z[id] { l.zPosition = CGFloat(neu); bekannt = true }
        }
        // zPosition allein hat sich als unzuverlaessig erwiesen, sobald die
        // Ebenen zwischendurch neu eingehaengt werden. Die REIHENFOLGE der
        // Unterebenen ist die Wahrheit, die CoreAnimation immer befolgt.
        if bekannt {
            let sortiert = ansicht.fensterEbenen
                .sorted { $0.value.zPosition < $1.value.zPosition }
                .map { $0.value }
            for l in sortiert { l.removeFromSuperlayer(); ansicht.buehne.addSublayer(l) }
        }
        CATransaction.commit()
        verdeckungPruefen()
    }
}

/// Welche Fenster sind im Kompositionsstapel VOLLSTAENDIG von hoeheren
/// Fenstern verdeckt? Nur Geometrie + zPosition (keine AX-, keine
/// Space-Abfrage). 7x7-Stichprobe je Fenster.
@MainActor func verdeckungPruefen() {
    var ergebnis: [UInt32: Bool] = [:]
    let sichtbar = ansicht.fensterEbenen.filter { !$0.value.isHidden }
    for (id, l) in sichtbar {
        guard let r = ansicht.orte[id], r.width > 4, r.height > 4 else { ergebnis[id] = false; continue }
        let darueber = sichtbar.filter { $0.key != id && $0.value.zPosition > l.zPosition }
            .compactMap { ansicht.orte[$0.key] }
        var alle = !darueber.isEmpty
        if alle {
            outer: for i in 0..<7 { for j in 0..<7 {
                let p = CGPoint(x: r.minX + 2 + (r.width - 4) * CGFloat(i) / 6,
                                y: r.minY + 2 + (r.height - 4) * CGFloat(j) / 6)
                if !darueber.contains(where: { $0.contains(p) }) { alle = false; break outer }
            } }
        }
        ergebnis[id] = alle
    }
    let n = ergebnis.values.filter { $0 }.count
    if n != letzteVerdeckte { letzteVerdeckte = n; log("verdeckt \(n)/\(ergebnis.count)") }
    let oben = sichtbar.max { $0.value.zPosition < $1.value.zPosition }?.key
    // TEMPORARY audit: the visual top window of the Miniatur.
    if oben != letztesOben { letztesOben = oben; sendeText("OBEN \(oben ?? 0)") }
    Task { await aufnahme.verdeckungSetzen(ergebnis, vorn: oben) }
}
var letzteVerdeckte = -1
var letztesOben: UInt32? = nil

/// Lage und Groesse der aufgenommenen Fenster nachziehen. Gemessen: ein
/// Fenster, das beim Start im Vollbild stand und es danach verliess, blieb in
/// der Miniatur bildschirmgross - gezeichnet UND getroffen an falscher Stelle.
/// Eine Fensterliste je 0,4 s; geaendert wird nur, was sich wirklich bewegt hat.
let geometrieQueue = DispatchQueue(label: "noki.schirm.geometrie", qos: .userInitiated)
var geometrieLaeuft = false
func geometrieNachziehen() {
    let ids = Set(ansicht.fensterEbenen.keys)
    guard !ids.isEmpty, !geometrieLaeuft else { return }
    // Nie waehrend einer Groessenanimation der Miniatur: die Abfrage haelt
    // die WindowServer-Verbindung, und der Animations-Commit wartete darauf.
    if Date().timeIntervalSince(hoverZeit) < 0.45 { return }
    // Die WindowServer-Abfrage laeuft NIE auf dem Hauptfaden: gemessen lag
    // sie dort bei 35 % seiner Zeit (synchron wartend auf WindowServer) und
    // verzoegerte Hover, Klicks und Tippen.
    geometrieLaeuft = true
    geometrieQueue.async { geometrieAbfragen(ids) }
}
func geometrieAbfragen(_ ids: Set<UInt32>) {
    defer { DispatchQueue.main.async { geometrieLaeuft = false } }
    // Nur die eigenen Fenster beschreiben lassen - nicht die ganze
    // Fensterliste (gemessen: 661 Fenster, ~18 ms je Abfrage, 2,5x/s).
    // Rohes CFArray der Fensternummern (ein Swift-Array "as CFArray" ergibt
    // Objekte statt Nummern - die Abfrage kam dann gemessen leer zurueck).
    var roh: [UnsafeRawPointer?] = ids.map { UnsafeRawPointer(bitPattern: UInt($0)) }
    let feld = CFArrayCreate(nil, &roh, roh.count, nil)
    let l = CGWindowListCreateDescriptionFromArray(feld) as? [[String: Any]] ?? []
    var neu: [UInt32: CGRect] = [:]
    for w in l {
        guard let n = (w[kCGWindowNumber as String] as? NSNumber)?.uint32Value, ids.contains(n),
              let b = w[kCGWindowBounds as String] as? [String: Any],
              let r = CGRect(dictionaryRepresentation: b as CFDictionary) else { continue }
        neu[n] = r
    }
    DispatchQueue.main.async {
        var geaendert = false, groesse = false
        for (id, r) in neu {
            guard let alt = ansicht.orte[id] else { continue }
            // Ein laufender oder gerade beendeter Zug gehoert dem Zeiger.
            if ansicht.lokalerZug?.wid == id { continue }
            if ansicht.zugSperreWid == id && Date() < ansicht.zugSperreBis { continue }
            if abs(alt.minX - r.minX) > 1 || abs(alt.minY - r.minY) > 1 || abs(alt.width - r.width) > 1 || abs(alt.height - r.height) > 1 {
                if abs(alt.width - r.width) > 1 || abs(alt.height - r.height) > 1 { groesse = true }
                ansicht.orte[id] = r
                geaendert = true
            }
        }
        if geaendert { ansicht.ordnen(); verdeckungPruefen() }
        if groesse { Task { await aufnahme.neuVermessen() } }
    }
}
Timer.scheduledTimer(withTimeInterval: 1.0, repeats: true) { _ in geometrieNachziehen() }

// Ereignisgesteuert, kein Takt: das Wechseln des vordersten Programms ist
// genau der Moment, in dem sich die Reihenfolge aendert.
NSWorkspace.shared.notificationCenter.addObserver(
    forName: NSWorkspace.didActivateApplicationNotification,
    object: nil, queue: .main
) { _ in
    // Die Meldung kommt, SOBALD das Programm aktiv wird - WindowServer
    // traegt die neue Reihenfolge aber erst kurz danach ein. Gemessen las
    // ein sofortiges Nachziehen noch die alte Lage, und die Miniatur hinkte
    // dadurch genau eine Aenderung hinterher. Deshalb einmal sofort und
    // zweimal kurz darauf; keine Schleife, nur diese drei Blicke.
    stapelNachziehen()
    for t in [0.12, 0.35] {
        DispatchQueue.main.asyncAfter(deadline: .now() + t) { stapelNachziehen() }
    }
}

_ = wischTap
NotificationCenter.default.addObserver(
    forName: NSWorkspace.activeSpaceDidChangeNotification,
    object: nil,
    queue: .main
) { _ in
    // Virtual-display mode has no physical "Noki Space".  A notification
    // here therefore always means the user changed a normal macOS Desktop.
    // Revalidate the live panel's membership immediately; this never
    // changes visibility intent, capture identity, or the virtual display.
    if nokiSpace == 0 {
        spacesRichten()
        log("space visibility revalidated activeDesktop user=true visible=\(sichtbar)")
    }
    pruefeNokiRichtung()
    // Sicherheitsnetz: ein verzerrter Overlay-Raum (z. B. Mission Control
    // ohne Dock-Meldung) wird hier erkannt und ersetzt.
    lageSichern("space_wechsel")
    guard let f_act = dlsym(cgsGriff, "CGSGetActiveSpace") else { return }
    let act = unsafeBitCast(f_act, to: (@convention(c) (Int32) -> UInt64).self)(cgsCid)
    _ = naechsteTransitionEpoch()
    if nokiSpace != 0 && act == nokiSpace {
        transitionSetzen(.stableOnNoki)
        // Ein verdeckter Fernvorgang ist kein Schreibtischwechsel des
        // Nutzers: er sieht weiter seinen Schreibtisch (Blende). Wuerde die
        // Miniatur hier "noki" annehmen, haengte sie sich um und nahme bis
        // zum Rueckweg keine Eingaben an - Klicks gingen verloren.
        if fernLaeuft { return }
        if ort != "noki" && ort != "noki_gewollt" {
            ortSetzen("noki", links: false, rechts: false)
        }
    } else if nokiSpace != 0 && act != nokiSpace {
        if ort == "noki" {
            transitionSetzen(.transitioningAway)
            ortSetzen("nutzer", links: nokiLinks, rechts: nokiRechts)
            transitionSetzen(.stableOffNoki)
        } else {
            transitionSetzen(.stableOffNoki)
            if wegGewischt || mitglied.alphaValue < 1 {
                wegGewischt = false
                if sichtbar { mitglied.alphaValue = 1 }
                anzeigen()
            }
        }
        if sichtbar {
            Task {
                await aufnahme.weiter()
                await aufnahme.frischePruefen()
            }
        }
    }
}
/// App launched/activated: Noki's user-desktop guard checks immediately
/// (a Dock/Spotlight launch may restore its window onto Noki's display).
for name in [NSWorkspace.didLaunchApplicationNotification, NSWorkspace.didActivateApplicationNotification] {
    NSWorkspace.shared.notificationCenter.addObserver(forName: name, object: nil, queue: .main) { n in
        let pid = (n.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication)?.processIdentifier ?? 0
        sendeText("APPAKTIV \(pid)")
        if n.name == NSWorkspace.didActivateApplicationNotification { lageSichern("app_aktiv") }
    }
}
for name in [NSWorkspace.didWakeNotification, NSWorkspace.screensDidWakeNotification] {
    NSWorkspace.shared.notificationCenter.addObserver(forName: name, object: nil, queue: .main) { n in
        let g = n.name == NSWorkspace.didWakeNotification ? "aufwachen" : "display_wach"
        // Nach dem Aufwach-Zoom des Systems pruefen (er dauert ~1 s).
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { lageSichern(g) }
    }
}
/// Bildschirm-Neuordnung: Grenzen der Noki-Anzeige frisch lesen und die
/// Stroeme neu beginnen. Entprellt - macOS meldet eine Neuordnung in Schueben.
var neuordnungItem: DispatchWorkItem?
NotificationCenter.default.addObserver(
    forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main
) { _ in
    neuordnungItem?.cancel()
    let item = DispatchWorkItem {
        if schreibtischAnzeige != 0 {
            let b = CGDisplayBounds(schreibtischAnzeige)
            if b.width > 0 && b.height > 0 && b != schreibtisch {
                log("anzeige \(schreibtischAnzeige) neu: \(schreibtisch) -> \(b)")
                schreibtisch = b
                sendeSchreibtisch()
                ansicht.needsLayout = true
            }
        }
        if vollAn { sollRahmen = vollRahmen(); anzeigen() }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { lageSichern("bildschirm") }
        Task { await aufnahme.neuStarten() }
    }
    neuordnungItem = item
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.5, execute: item)
}
if let w = ladeHintergrund() { ansicht.wand.contents = w }
Task { await aufnahme.hostZielSetzen(startIds); await aufnahme.setze(startIds) }
DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { ersteKompositionFertig("frist") }

// Leichter Abgleich, NUR solange die Miniatur gross ist (sonst ein
// einziger Vergleich und fertig). Faengt jedes verlorene mouseExited.
Timer.scheduledTimer(withTimeInterval: 0.2, repeats: true) { _ in
    if ansicht.istGross { hoverAbgleichen() }
}

var frischeTakt = 0
/// SHARPNESS invariant: a layer must never show a texture with far fewer
/// pixels than it is displayed at. A frozen hidden app never delivers the
/// larger frame after COMPACT -> LARGE, so the old small texture was
/// stretched - "the Miniatur gets blurry over time". The backing store has
/// the last painted content at full resolution; swap it in atomically.
var schaerfeZuletzt: [UInt32: Date] = [:]
var schaerfeKorrekturen = 0
@MainActor func schaerfePruefen() {
    let skala = ansicht.window?.backingScaleFactor ?? 2
    for (id, l) in ansicht.fensterEbenen where !l.isHidden && l.bounds.width > 8 {
        let breite: Int
        if let c = l.contents, CFGetTypeID(c as CFTypeRef) == IOSurfaceGetTypeID() {
            breite = IOSurfaceGetWidth(c as! IOSurfaceRef)
        } else if let c = l.contents, CFGetTypeID(c as CFTypeRef) == CGImage.typeID {
            breite = (c as! CGImage).width
        } else { continue }
        let noetig = l.bounds.width * skala
        guard CGFloat(breite) < noetig * 0.8 else { continue }
        if let t = schaerfeZuletzt[id], Date().timeIntervalSince(t) < 3 { continue }
        schaerfeZuletzt[id] = Date()
        DispatchQueue.global(qos: .utility).async {
            guard let bild = fensterStandbild(id), CGFloat(bild.width) >= noetig * 0.8 else { return }
            DispatchQueue.main.async {
                guard let l = ansicht.fensterEbenen[id] else { return }
                CATransaction.begin(); CATransaction.setDisableActions(true)
                l.contents = bild
                CATransaction.commit()
                schaerfeKorrekturen += 1
                log("schaerfe \(id) \(breite)px -> \(bild.width)px (need \(Int(noetig)))")
                sendeText("SCHAERFE \(id) \(breite) \(bild.width) \(Int(noetig))")
            }
        }
    }
}
/// Native order of the Noki Space as last applied (REAL_SPACE).
var letzteReihe: [UInt32] = []
// Bildrate messen - einmal je Sekunde, nur wenn sich etwas bewegt hat.
Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { _ in
    MainActor.assumeIsolated { schaerfePruefen() }
    // This timer runs on the compositor/main run loop. If UI presentation
    // freezes, PULSE stops even when the helper process itself still exists.
    // The Rust host first requests a stream rebuild, then replaces a helper
    // whose presentation loop remains silent. Works for an empty Desktop too.
    Task {
        let p = await aufnahme.puls()
        sendeText("PULSE streams \(p.streams) capture \(p.capture ? 1 : 0) generation \(p.generation)")
    }
    // One WindowServer call: re-stack only if the real order changed (e.g.
    // an app raised its own window, a dialog opened).
    if nokiSpace != 0 {
        let reihe = spaceReihe(nokiSpace).filter { ansicht.fensterEbenen[$0] != nil }
        if reihe != letzteReihe { letzteReihe = reihe; stapelNachziehen() }
    }
    let fpsJetzt = bilderZaehler
    if !fpsJetzt.isEmpty {
        let msg = "FPS " + fpsJetzt.map { "\($0.key):\($0.value)" }.sorted().joined(separator: " ")
        sendeText(msg)
        log(msg)
    }
    bilderZaehler.removeAll()
    let interaction: String = ansicht.zieht ? "DRAG"
        : (ansicht.rechtsZiel != nil ? "CONTEXT_MENU" : (ansicht.tippen ? "TYPING" : "IDLE"))
    frischeTakt += 1
    Task {
        // Diagnose-Zeilen nur alle 10 s (vorher 13 Zeilen je Sekunde an Noki);
        // die Stall-Pruefung selbst laeuft weiter jede Sekunde.
        let zeilen = frischeTakt % 10 == 0
            ? await aufnahme.frischeZeilen(fps: fpsJetzt, interaction: interaction, workerBusy: fernLaeuft) : []
        for z in zeilen {
            log(z)
            // Temporary end-to-end freshness instrumentation: route the same
            // compact line through the existing stdout protocol so the host's
            // production runtime log records source/compositor/publication
            // progress even when the helper was launched from Finder.
            sendeText(z)
        }
        await aufnahme.frischePruefen()
    }
}

func zahlen(_ s: String) -> [Double] { s.split(separator: " ").compactMap { Double($0) } }

// Befehle von Noki lesen. Endet stdin, endet der Prozess - so bleibt kein
// Aufnahmeprozess zurueck, wenn Noki verschwindet.
DispatchQueue.global(qos: .userInitiated).async {
    while let zeile = readLine(strippingNewline: true) {
        let teile = zeile.split(separator: " ", maxSplits: 1).map(String.init)
        let rest = teile.count > 1 ? teile[1] : ""
        switch teile.first ?? "" {
        case "set", "set!":
            let ids = rest.split(separator: ",").compactMap { UInt32($0) }
            let erzwingen = teile.first == "set!"
            // Only un-pause: `weiter()` would first revive and REPUBLISH the
            // paused (old) composition, and its READY was credited to the new
            // target - an empty target Desktop then showed the user's own
            // Desktop under the new label (recursive Miniatur).
            Task { await aufnahme.entpausieren(); await aufnahme.hostZielSetzen(ids); await aufnahme.setze(ids, erzwingen: erzwingen) }
        case "pause":  Task { await aufnahme.pause() }
        case "resume": Task { await aufnahme.weiter() }
        case "rahmen":
            let z = zahlen(rest)
            guard z.count == 4 else { break }
            DispatchQueue.main.async {
                let neu = CGRect(x: z[0], y: z[1], width: z[2], height: z[3])
                // Leerer Rahmen = ausdrueckliches Verbergen (Shortcut 4). Es
                // gewinnt IMMER gegen Hover/LARGE/Vollansicht: frueher wurde
                // daraus bei `istGross` (Zeiger in der Miniatur) wieder der
                // grosse Rahmen - die Miniatur blieb offen.
                if neu.width < 2 || neu.height < 2 {
                    ansicht.hoverWorkItem?.cancel()
                    ansicht.hoverWorkItem = nil
                    let warGross = ansicht.istGross
                    ansicht.istGross = false
                    if vollAn { vollAn = false; ansicht.navi.titel = naviText; ansicht.navi.pfeil = true; sendeText("VOLL aus") }
                    sollRahmen = .zero
                    anzeigen(animiert: false)
                    if warGross { sendeText("HOVER kompakt") }
                    return
                }
                // Echo der eigenen Lage (die Oberflaeche meldet ihre Zone nach
                // jedem Hover zurueck): nichts Neues - KEINE zweite Animation.
                // Gemessen unterbrach sie die laufende Hover-Kurve ~100 ms spaeter.
                if neu == kompaktRahmen && !vollAn && sollRahmen == (ansicht.istGross ? grossRahmen(kompaktRahmen) : kompaktRahmen) {
                    hoverAbgleichen()
                    return
                }
                if neu.width >= 2 && (!ansicht.istGross || kompaktRahmen == .zero) {
                    if kompaktRahmen.width >= 2 && kompaktRahmen.size != neu.size {
                        sendeText("KOMPAKT_NEU \(Int(kompaktRahmen.width))x\(Int(kompaktRahmen.height))->\(Int(neu.width))x\(Int(neu.height))")
                    }
                    kompaktRahmen = neu
                }
                let alt = sollRahmen.size
                if vollAn && neu.width >= 2 {
                    // Die Oberflaeche meldet weiter ihre Kompaktlage; die
                    // Vollansicht bleibt, bis sie geschlossen wird.
                    return
                }
                if vollAn { vollAn = false; ansicht.navi.titel = naviText; ansicht.navi.pfeil = true; sendeText("VOLL aus") }
                sollRahmen = ansicht.istGross ? grossRahmen(kompaktRahmen) : neu
                anzeigen(animiert: true)
                if alt != sollRahmen.size && sollRahmen.width >= 2 {
                    Task { await aufnahme.neuVermessen() }
                }
                // Trefferzone/Frame wurde gerade neu gebaut: dieselbe
                // autoritative Ortspruefung wie nach Ferninteraktionen.
                hoverAbgleichen()
            }
        case "desktop":
            let z = zahlen(rest)
            guard z.count >= 4, z[2] > 0, z[3] > 0 else { break }
            DispatchQueue.main.async {
                if z.count >= 5 { schreibtischAnzeige = CGDirectDisplayID(z[4]) }
                schreibtisch = CGRect(x: z[0], y: z[1], width: z[2], height: z[3])
                sendeSchreibtisch()
                ansicht.needsLayout = true
                Task { await aufnahme.neuVermessen() }
            }
        case "messen":
            DispatchQueue.main.async { miniaturMessen(rest.isEmpty ? "befehl" : rest) }
        case "geometrie":
            let z = rest.split(separator: " ").compactMap { Double($0) }
            guard z.count == 5 else { break }
            DispatchQueue.main.async {
                let wid = UInt32(z[0])
                // Waehrend der Helfer den Zug selbst fuehrt, hinken Nokis
                // Zwischenstaende hinterher - nur der Endstand zaehlt.
                if ansicht.lokalerZug?.wid == wid { return }
                let neu = CGRect(x: z[1], y: z[2], width: z[3], height: z[4])
                if let alt = ansicht.orte[wid], abs(alt.width - neu.width) > 1 || abs(alt.height - neu.height) > 1 {
                    vermessenPlanen(grund: "resize_end")
                }
                ansicht.orte[wid] = neu
                if ansicht.zugSperreWid == wid { ansicht.zugSperreBis = .distantPast }
                ansicht.ordnen()
            }
        case "zug":
            // zug <wid> <16|Kantenmaske> <x> <y> <fx> <fy> <fw> <fh>  (Beginn)
            // zug <wid> -1   (kein Fensterzug: Auswahl)   zug <wid> 0 (Ende)
            let z = rest.split(separator: " ").compactMap { Double($0) }
            guard z.count >= 2 else { break }
            DispatchQueue.main.async {
                let wid = UInt32(z[0]), art = Int(z[1])
                if art > 0, z.count == 8, let g = ansicht.gedruecktesFernziel, g.wid == wid {
                    ansicht.lokalerZug = (wid, art, CGPoint(x: z[2], y: z[3]),
                                          CGRect(x: z[4], y: z[5], width: z[6], height: z[7]))
                    ansicht.zeigerAbgleichen()
                } else if art < 0, ansicht.lokalerZug?.wid == wid, ansicht.lokalerZug?.art == 16 {
                    ansicht.lokalerZug = nil
                } else if art == 0, ansicht.lokalerZug?.wid == wid {
                    // End of a move/resize: the REAL frame (Noki sends it
                    // right after) must win. Before, `zug 0` was ignored, the
                    // local drag stayed active and the real frame was dropped -
                    // Spotify (min width 800) stayed drawn at the smaller
                    // requested size: content cut at the side.
                    ansicht.lokalerZug = nil
                }
            }
        case "ziehbar":
            let z = rest.split(separator: " ").compactMap { Double($0) }
            guard z.count == 4 else { break }
            DispatchQueue.main.async {
                ansicht.kopfAntwort = (UInt32(z[0]), CGPoint(x: z[1], y: z[2]), z[3] == 1)
                if ansicht.gedruecktesFernziel == nil { ansicht.zeigerAbgleichen() }
            }
        case "hinweis_fenster":
            let t = rest.components(separatedBy: "\t")
            guard t.count == 3 else { break }
            DispatchQueue.main.async {
                let bild = t[0].hasSuffix(".app") ? NSWorkspace.shared.icon(forFile: t[0]) : nil
                ansicht.hinweisZeigen(bild: bild, name: t[1].replacingOccurrences(of: "Google ", with: ""), titel: t[2])
            }
        case "label":
            DispatchQueue.main.async { ansicht.langText = rest }
        // Die Nummer kommt aus der laufenden Ordnung, nicht aus dem Code.
        case "ziel_meta":
            // Label + app bar of the NEXT target: held back and applied in
            // the same main-thread swap as the new composition (one
            // generation - never the new picture with the old app bar).
            let d = (try? JSONSerialization.jsonObject(with: Data(rest.utf8))) as? [String: Any] ?? [:]
            DispatchQueue.main.async {
                wartendeZielMeta = (d["navi"] as? String ?? "", d["leiste"] as? [[String: Any]] ?? [])
            }
        case "navi":
            DispatchQueue.main.async {
                if zielWechselLaeuft {
                    wartendeNavi = rest.isEmpty ? "Zum Schreibtisch" : rest
                    return
                }
                naviText = rest.isEmpty ? "Zum Schreibtisch" : rest
                if !vollAn { ansicht.navi.titel = naviText }
                ansicht.needsLayout = true
            }
        case "modus":
            DispatchQueue.main.async {
                // Die DOM-Seite spiegelt den nativen Stand zurueck. Reale
                // Zeigergeometrie bleibt der einzige Groessenbesitzer.
                hoverAbgleichen()
            }
        case "mc":
            // Mission Control offen/zu (Dock-AX-Meldung aus Noki).
            let offen = rest == "1"
            DispatchQueue.main.async { missionControl(offen) }
        case "noki_space":
            let ns = UInt64(rest) ?? 0
            DispatchQueue.main.async {
                nokiSpace = ns
                pruefeNokiRichtung()
                stapelNachziehen()
            }
        case "spaces":
            let s = rest.split(separator: ",").compactMap { UInt64($0) }
            DispatchQueue.main.async { sollSpaces = s; spacesRichten() }
        case "ort":
            // ort <nutzer|noki|noki_gewollt> <noki links 0|1> <noki rechts 0|1>
            let z = rest.split(separator: " ").map(String.init)
            guard z.count == 3 else { break }
            DispatchQueue.main.async {
                ortSetzen(z[0], links: z[1] == "1", rechts: z[2] == "1")
                // Ein verdeckter Fern-Hin/Rueckweg darf die Groesse nicht
                // besitzen. Nach Ortsaenderungen gilt sofort derselbe
                // reale Zeiger-vs-Rahmen-Abgleich.
                ansicht.hoverGesperrt = false
                hoverAbgleichen()
            }
        case "fern":
            // Nur Lebenszeichen/Notbremse; Fernbedienung ist KEIN Hover-
            // Besitz. Ausschliesslich eine wirklich gedrueckte Maustaste
            // darf das Einklappen bis mouseUp verschieben.
            let an = rest == "an"
            DispatchQueue.main.async {
                fernRunde += 1
                fernLaeuft = an
                // Die Rueckkehr ist gemeldet, bevor das Panel wieder
                // Ereignisse bekommt; kurz danach faengt der Tap noch.
                if !an { fernNachlaufBis = Date().addingTimeInterval(0.3) }
                if an {
                    let r = fernRunde
                    hoverAbgleichen()
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) {
                        if fernRunde == r { hoverAbgleichen() }
                    }
                } else {
                    hoverAbgleichen()
                    // Und noch einmal, wenn sich alles gesetzt hat.
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) { hoverAbgleichen() }
                }
            }
        case "voll":
            let an = rest == "an"
            DispatchQueue.main.async { vollSetzen(an) }
        case "tap":
            // Klick, den Noki selbst abgefangen hat (offenes echtes Menue):
            // derselbe Klickweg der Ansicht wie ein direkter Klick.
            let z = rest.split(separator: " ").map(String.init)
            guard z.count == 3, let x = Double(z[1]), let y = Double(z[2]) else { break }
            DispatchQueue.main.async {
                guard let w = ansicht.window else { return }
                let hoehe = NSScreen.screens.first?.frame.height ?? 0
                let p = ansicht.convert(w.convertPoint(fromScreen: NSPoint(x: x, y: hoehe - y)), from: nil)
                if z[0] == "ab" { ansicht.druckBeginn(p, klicks: 1) } else { ansicht.druckEnde(p) }
            }
        case "leiste":
            let l = (try? JSONSerialization.jsonObject(with: Data(rest.utf8))) as? [[String: Any]] ?? []
            DispatchQueue.main.async { ansicht.leisteSetzen(l) }
        case "katalog":
            let l = (try? JSONSerialization.jsonObject(with: Data(rest.utf8))) as? [[String: Any]] ?? []
            DispatchQueue.main.async { ansicht.katalogZeigen(l) }
        case "fokusring":
            let z = zahlen(rest)
            DispatchQueue.main.async { ansicht.fokusRechteck = z.count == 4 ? CGRect(x: z[0], y: z[1], width: z[2], height: z[3]) : nil }
        case "caret":
            let z = zahlen(rest)
            DispatchQueue.main.async { ansicht.caretRechteck = z.count == 4 ? CGRect(x: z[0], y: z[1], width: z[2], height: z[3]) : nil }
        case "markierung":
            let r: [CGRect] = rest.split(separator: ";").compactMap {
                let z = zahlen(String($0)); return z.count == 4 ? CGRect(x: z[0], y: z[1], width: z[2], height: z[3]) : nil
            }
            DispatchQueue.main.async { ansicht.markierungSetzen(r) }
        case "hinweis":
            DispatchQueue.main.async { ansicht.hinweisZeigen(rest) }
        case "tippen":
            let an = rest == "an"
            DispatchQueue.main.async { ansicht.tippen = an }
        case "interaction":
            let an = rest == "an"
            DispatchQueue.main.async { ansicht.interaction = an }
        case "testrad":
            // testrad <wid> <rx> <ry> <profil> <richtung +1 down/-1 up>
            let z = rest.split(separator: " ").map(String.init)
            guard z.count >= 5, let w = UInt32(z[0]), let rx = Double(z[1]), let ry = Double(z[2]) else { break }
            DispatchQueue.main.async { testGeste(w, CGFloat(rx), CGFloat(ry), z[3], Double(z[4]) ?? 1) }
        case "rollmessung":
            let z = rest.split(separator: " ")
            DispatchQueue.main.async { rollMessung(UInt32(z.first ?? "") ?? 0, z.count > 1 && z[1] == "an") }
        case "lebhaft":
            // Noki stellt gerade Eingaben an dieses Fenster zu (Tippen).
            if let w = UInt32(rest) { Task { await aufnahme.erwarteAenderung(w) } }
        case "vorn":
            let w = UInt32(rest) ?? 0
            DispatchQueue.main.async { kompositionVorn = w; stapelNachziehen() }
        case "fenstervorn":
            let z = rest.split(separator: " ").compactMap { Int64($0) }
            guard z.count == 2 else { break }
            DispatchQueue.main.async { let ok = fensterVordergrund(pid_t(z[0]), UInt32(z[1])); sendeText("FENSTERVORN \(z[1]) \(ok)") }
        case "zustand":
            // Consistency audit: EXACTLY what the Miniatur draws and hit-tests
            // with right now (front-to-back), plus per-window frame age.
            Task {
                let alter = await aufnahme.bildAlter()
                let gen = await aufnahme.bildGeneration()
                DispatchQueue.main.async {
                    let reihe = ansicht.fensterEbenen.filter { !$0.value.isHidden }
                        .sorted { $0.value.zPosition > $1.value.zPosition }
                    let skala = ansicht.window?.backingScaleFactor ?? 2
                    let teile = reihe.map { (id, l) -> String in
                        let r = ansicht.orte[id] ?? .zero
                        let a = alter[id].map { String(format: "%.1f", $0) } ?? "-"
                        var tex = 0
                        if let c = l.contents, CFGetTypeID(c as CFTypeRef) == IOSurfaceGetTypeID() { tex = IOSurfaceGetWidth(c as! IOSurfaceRef) }
                        else if let c = l.contents, CFGetTypeID(c as CFTypeRef) == CGImage.typeID { tex = (c as! CGImage).width }
                        let noetig = Int(l.bounds.width * skala)
                        return "\(id):\(Int(r.minX)),\(Int(r.minY)),\(Int(r.width)),\(Int(r.height)):\(a):\(tex)/\(noetig):\(gen[id] ?? 0)"
                    }
                    sendeText("ZUSTAND space=\(nokiSpace) " + teile.joined(separator: " "))
                }
            }
        case "stapel":
            stapelNachziehen()
            for t in [0.12, 0.35] {
                DispatchQueue.main.asyncAfter(deadline: .now() + t) { stapelNachziehen() }
            }
        case "neustart":
            Task { await aufnahme.neuStarten() }
        case "abdecken":
            DispatchQueue.main.async { abdecken() }
        case "aufdecken":
            let s = rest.split(separator: ",").compactMap { UInt64($0) }
            DispatchQueue.main.async { aufdecken(rest.isEmpty ? nil : s) }
        case "quit":   exit(0)
        default: break
        }
    }
    exit(0)
}
app.run()
