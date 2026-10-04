// Noki Kamera - Medien-Helfer (kurzlebig, ein Aufruf = eine Aufgabe).
//
//   noki-medien thumb  <datei> <ziel.jpg> <px>
//       Vorschaubild (Foto: ImageIO-Thumbnail, Video: ein Frame) als JPEG.
//       stdout: {"w":..,"h":..,"dauer":..}            (dauer nur bei Video)
//   noki-medien bild   <bearbeitet.png> <ziel> <original>
//       Schreibt das bearbeitete Bild im FORMAT des Originals (PNG/JPEG/HEIC
//       …) und uebernimmt dessen Metadaten, soweit sinnvoll (EXIF/TIFF/GPS,
//       Farbprofil; Groesse und Ausrichtung kommen vom neuen Bild).
//   noki-medien export <video> <ziel.mov> <start> <ende> <x> <y> <w> <h> [<ebene.png> <t0>]…
//       Zeitlich schneiden (Sekunden) und raeumlich zuschneiden (0..1 der
//       angezeigten Flaeche). Ebenen (Zeichnungen/Texte des Editors, PNG in
//       Groesse des Zuschnitts) werden ab t0 (Sekunden der Quelle) bis zum
//       Ende eingebrannt. Ohne Zuschnitt und Ebenen verlustfrei (Passthrough).
//       stdout: {"p":0.42} … {"ok":true} | {"fehler":"…"}
//       SIGTERM bricht ab und entfernt die halbe Datei.
// Alles lokal (ImageIO/AVFoundation/CoreImage), nichts verlaesst diesen Mac.
import Foundation
import AVFoundation
import ImageIO
import CoreGraphics
import UniformTypeIdentifiers
import QuartzCore

setvbuf(stdout, nil, _IOLBF, 0)
func emit(_ d: [String: Any]) {
    if let j = try? JSONSerialization.data(withJSONObject: d), let s = String(data: j, encoding: .utf8) { print(s); fflush(stdout) }
}
func fehler(_ s: String) -> Never { emit(["fehler": s]); exit(1) }

let a = CommandLine.arguments
guard a.count >= 2 else { fehler("aufruf") }

func istVideo(_ url: URL) -> Bool {
    ["mov", "mp4", "m4v"].contains(url.pathExtension.lowercased())
}
func jpegSchreiben(_ bild: CGImage, _ ziel: URL) -> Bool {
    guard let d = CGImageDestinationCreateWithURL(ziel as CFURL, UTType.jpeg.identifier as CFString, 1, nil) else { return false }
    CGImageDestinationAddImage(d, bild, [kCGImageDestinationLossyCompressionQuality: 0.78] as CFDictionary)
    return CGImageDestinationFinalize(d)
}
func anzeigeGroesse(_ spur: AVAssetTrack) -> CGSize {
    let r = CGRect(origin: .zero, size: spur.naturalSize).applying(spur.preferredTransform)
    return CGSize(width: abs(r.width), height: abs(r.height))
}

switch a[1] {
case "thumb":
    guard a.count == 5, let px = Int(a[4]) else { fehler("aufruf") }
    let quelle = URL(fileURLWithPath: a[2]), ziel = URL(fileURLWithPath: a[3])
    if istVideo(quelle) {
        let asset = AVURLAsset(url: quelle)
        let dauer = CMTimeGetSeconds(asset.duration)
        guard let spur = asset.tracks(withMediaType: .video).first else { fehler("keine Videospur") }
        let g = AVAssetImageGenerator(asset: asset)
        g.appliesPreferredTrackTransform = true
        g.maximumSize = CGSize(width: px, height: px)
        g.requestedTimeToleranceBefore = CMTime(seconds: 0.5, preferredTimescale: 600)
        g.requestedTimeToleranceAfter = CMTime(seconds: 0.5, preferredTimescale: 600)
        let t = CMTime(seconds: dauer.isFinite ? min(1.0, dauer * 0.1) : 0, preferredTimescale: 600)
        guard let bild = try? g.copyCGImage(at: t, actualTime: nil), jpegSchreiben(bild, ziel) else { fehler("frame") }
        let s = anzeigeGroesse(spur)
        emit(["w": Int(s.width), "h": Int(s.height), "dauer": dauer.isFinite ? dauer : 0])
    } else {
        guard let src = CGImageSourceCreateWithURL(quelle as CFURL, nil) else { fehler("bild") }
        let opt: [CFString: Any] = [kCGImageSourceCreateThumbnailFromImageAlways: true, kCGImageSourceCreateThumbnailWithTransform: true,
                                    kCGImageSourceThumbnailMaxPixelSize: px, kCGImageSourceShouldCacheImmediately: false]
        guard let bild = CGImageSourceCreateThumbnailAtIndex(src, 0, opt as CFDictionary), jpegSchreiben(bild, ziel) else { fehler("thumbnail") }
        let p = CGImageSourceCopyPropertiesAtIndex(src, 0, nil) as? [CFString: Any] ?? [:]
        var w = p[kCGImagePropertyPixelWidth] as? Int ?? 0, h = p[kCGImagePropertyPixelHeight] as? Int ?? 0
        if let o = p[kCGImagePropertyOrientation] as? Int, o >= 5 { swap(&w, &h) }
        emit(["w": w, "h": h])
    }

case "bild":
    guard a.count == 5 else { fehler("aufruf") }
    let neu = URL(fileURLWithPath: a[2]), ziel = URL(fileURLWithPath: a[3]), original = URL(fileURLWithPath: a[4])
    guard let nsrc = CGImageSourceCreateWithURL(neu as CFURL, nil), let bild = CGImageSourceCreateImageAtIndex(nsrc, 0, nil) else { fehler("bearbeitetes Bild") }
    let osrc = CGImageSourceCreateWithURL(original as CFURL, nil)
    var typ = UTType.png.identifier
    var meta: [CFString: Any] = [:]
    if let o = osrc {
        if let t = CGImageSourceGetType(o) { typ = t as String }
        if let p = CGImageSourceCopyPropertiesAtIndex(o, 0, nil) as? [CFString: Any] { meta = p }
    }
    // Neue Pixel: Groesse und Ausrichtung gehoeren zum bearbeiteten Bild.
    for k in [kCGImagePropertyPixelWidth, kCGImagePropertyPixelHeight, kCGImagePropertyOrientation] { meta.removeValue(forKey: k) }
    if var tiff = meta[kCGImagePropertyTIFFDictionary] as? [CFString: Any] { tiff.removeValue(forKey: kCGImagePropertyTIFFOrientation); meta[kCGImagePropertyTIFFDictionary] = tiff }
    if var exif = meta[kCGImagePropertyExifDictionary] as? [CFString: Any] {
        exif[kCGImagePropertyExifPixelXDimension] = bild.width; exif[kCGImagePropertyExifPixelYDimension] = bild.height
        meta[kCGImagePropertyExifDictionary] = exif
    }
    if typ != UTType.png.identifier { meta[kCGImageDestinationLossyCompressionQuality] = 0.92 }
    let tmp = ziel.deletingLastPathComponent().appendingPathComponent(".noki-\(getpid())-" + ziel.lastPathComponent)
    guard let d = CGImageDestinationCreateWithURL(tmp as CFURL, typ as CFString, 1, nil) else { fehler("format") }
    CGImageDestinationAddImage(d, bild, meta as CFDictionary)
    guard CGImageDestinationFinalize(d) else { try? FileManager.default.removeItem(at: tmp); fehler("schreiben") }
    // Erst vollstaendig schreiben, dann an den Zielort (nie eine halbe Datei).
    try? FileManager.default.removeItem(at: ziel)
    do { try FileManager.default.moveItem(at: tmp, to: ziel) } catch { try? FileManager.default.removeItem(at: tmp); fehler("\(error)") }
    emit(["ok": true, "w": bild.width, "h": bild.height])

case "export":
    guard a.count >= 10, (a.count - 10) % 2 == 0, let start = Double(a[4]), let ende = Double(a[5]),
          let cx = Double(a[6]), let cy = Double(a[7]), let cw = Double(a[8]), let ch = Double(a[9]) else { fehler("aufruf") }
    let quelle = URL(fileURLWithPath: a[2]), ziel = URL(fileURLWithPath: a[3])
    // Ebenen: (Bild, t0)
    var ebenen: [(CGImage, Double)] = []
    var i = 10
    while i + 1 < a.count {
        if let t0 = Double(a[i + 1]), let src = CGImageSourceCreateWithURL(URL(fileURLWithPath: a[i]) as CFURL, nil),
           let bild = CGImageSourceCreateImageAtIndex(src, 0, nil) { ebenen.append((bild, t0)) }
        i += 2
    }
    let asset = AVURLAsset(url: quelle)
    let gesamt = CMTimeGetSeconds(asset.duration)
    let von = max(0, min(start, gesamt)), bis = max(von, min(ende, gesamt))
    guard bis - von > 0.05 else { fehler("zu kurz") }
    let bereich = CMTimeRange(start: CMTime(seconds: von, preferredTimescale: 600), end: CMTime(seconds: bis, preferredTimescale: 600))
    // Der Schnitt als eigene Komposition: deren Zeitachse beginnt bei 0 -
    // genau die Zeit, in der auch die Ebenen eingeblendet werden.
    let komp = AVMutableComposition()
    let quellSpur = asset.tracks(withMediaType: .video).first
    var kompSpur: AVMutableCompositionTrack?
    if let sp = quellSpur, let k = komp.addMutableTrack(withMediaType: .video, preferredTrackID: kCMPersistentTrackID_Invalid) {
        do { try k.insertTimeRange(bereich, of: sp, at: .zero) } catch { fehler("schnitt") }
        k.preferredTransform = sp.preferredTransform
        kompSpur = k
    }
    for sp in asset.tracks(withMediaType: .audio) {
        if let k = komp.addMutableTrack(withMediaType: .audio, preferredTrackID: kCMPersistentTrackID_Invalid) { try? k.insertTimeRange(bereich, of: sp, at: .zero) }
    }
    let ganz = cx <= 0.0005 && cy <= 0.0005 && cw >= 0.9995 && ch >= 0.9995
    let einfach = ganz && ebenen.isEmpty
    guard let s = AVAssetExportSession(asset: komp, presetName: einfach || kompSpur == nil ? AVAssetExportPresetPassthrough : AVAssetExportPresetHighestQuality) else { fehler("export") }
    s.outputURL = ziel
    s.outputFileType = .mov
    if !einfach, let sp = quellSpur, let k = kompSpur {
        let anzeige = anzeigeGroesse(sp)
        // Gerade Pixelzahlen (H.264/HEVC), mindestens 16 px.
        let rw = max(16, (Int(anzeige.width * cw) / 2) * 2), rh = max(16, (Int(anzeige.height * ch) / 2) * 2)
        let ox = anzeige.width * cx, oy = anzeige.height * cy
        let comp = AVMutableVideoComposition()
        comp.renderSize = CGSize(width: rw, height: rh)
        let fps = sp.nominalFrameRate > 0 ? sp.nominalFrameRate : 30
        comp.frameDuration = CMTime(value: 1, timescale: CMTimeScale(fps.rounded()))
        let anw = AVMutableVideoCompositionInstruction()
        anw.timeRange = CMTimeRange(start: .zero, duration: komp.duration)
        let lage = AVMutableVideoCompositionLayerInstruction(assetTrack: k)
        lage.setTransform(sp.preferredTransform.concatenating(CGAffineTransform(translationX: -ox, y: -oy)), at: .zero)
        anw.layerInstructions = [lage]
        comp.instructions = [anw]
        if !ebenen.isEmpty {
            // Jede Ebene als Bild-Layer ueber dem Video, unsichtbar bis t0.
            let rahmen = CGRect(x: 0, y: 0, width: rw, height: rh)
            let eltern = CALayer(), videoEbene = CALayer()
            eltern.frame = rahmen; videoEbene.frame = rahmen
            eltern.addSublayer(videoEbene)
            for (bild, t0) in ebenen {
                let l = CALayer()
                l.frame = rahmen
                l.contents = bild
                l.contentsGravity = .resize
                let ab = max(0, t0 - von)
                if ab > 0.001 {
                    l.opacity = 0
                    let an = CABasicAnimation(keyPath: "opacity")
                    an.fromValue = 1; an.toValue = 1
                    an.beginTime = AVCoreAnimationBeginTimeAtZero + ab
                    an.duration = max(0.05, CMTimeGetSeconds(komp.duration) - ab + 1)
                    an.fillMode = .forwards
                    an.isRemovedOnCompletion = false
                    l.add(an, forKey: "ab")
                }
                eltern.addSublayer(l)
            }
            comp.animationTool = AVVideoCompositionCoreAnimationTool(postProcessingAsVideoLayer: videoEbene, in: eltern)
        }
        s.videoComposition = comp
    }
    try? FileManager.default.removeItem(at: ziel)
    // Abbruch (SIGTERM von Noki): Export stoppen, halbe Datei weg.
    signal(SIGTERM, SIG_IGN)
    let abbruch = DispatchSource.makeSignalSource(signal: SIGTERM, queue: .main)
    abbruch.setEventHandler { s.cancelExport() }
    abbruch.resume()
    let takt = DispatchSource.makeTimerSource(queue: .main)
    takt.schedule(deadline: .now(), repeating: .milliseconds(250))
    takt.setEventHandler { emit(["p": Double(s.progress)]) }
    takt.resume()
    s.exportAsynchronously {
        DispatchQueue.main.async {
            takt.cancel()
            switch s.status {
            case .completed: emit(["p": 1.0]); emit(["ok": true]); exit(0)
            case .cancelled: try? FileManager.default.removeItem(at: ziel); emit(["abgebrochen": true]); exit(2)
            default: try? FileManager.default.removeItem(at: ziel); emit(["fehler": s.error?.localizedDescription ?? "export"]); exit(1)
            }
        }
    }
    dispatchMain()

default:
    fehler("unbekannt")
}
