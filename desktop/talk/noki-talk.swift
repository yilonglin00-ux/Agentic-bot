// Noki Talk - Aufnahme-Helfer (dauerhaft, kein Prozessstart pro Diktat).
//
// Laeuft nach dem ersten Diktat weiter und wartet auf stdin. Das Mikrofon
// ist NUR zwischen "start" und "stop"/"cancel" offen; dazwischen gibt es
// keinen Audio-Takt, kein Polling, keine CPU-Last.
//
// stdin  (Zeilen): "start <datei.m4a>" | "stop" | "cancel" | "status"
// stdout (JSON-Zeilen):
//   {"ready":true,"mic":<0..3>,"device":"..."}         beim Start / auf "status"
//   {"state":"recording","device":"...","rate":48000}  Mikrofon laeuft
//   {"pcm":"<base64 Int16 LE, 16 kHz mono>","level":0.42}
//   {"state":"stopped","kept":true,"frames":12345}     Datei geschlossen
//   {"error":"mic_denied"|"engine"|"file", ...}
// Audio verlaesst diesen Mac nie: PCM geht ueber die lokale Pipe an Noki,
// die eine Verlaufsdatei (AAC, 16 kHz mono) liegt in Nokis Datenordner.
import Foundation
import AVFoundation

setvbuf(stdout, nil, _IOLBF, 0)
let ausgabe = NSLock()
func emit(_ d: [String: Any]) {
    ausgabe.lock(); defer { ausgabe.unlock() }
    if let j = try? JSONSerialization.data(withJSONObject: d), let s = String(data: j, encoding: .utf8) { print(s); fflush(stdout) }
}
func geraet() -> String { AVCaptureDevice.default(for: .audio)?.localizedName ?? "" }

let ziel = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 16000, channels: 1, interleaved: false)!
let schreiben = DispatchQueue(label: "noki.talk.write")
var engine: AVAudioEngine?
var datei: AVAudioFile?
var dateiPfad = ""
var frames = 0

func freigabe() -> Bool {
    switch AVCaptureDevice.authorizationStatus(for: .audio) {
    case .authorized: return true
    case .notDetermined:
        let sem = DispatchSemaphore(value: 0); var ok = false
        AVCaptureDevice.requestAccess(for: .audio) { ok = $0; sem.signal() }
        sem.wait(); return ok
    default: return false
    }
}

func starten(_ pfad: String) {
    if engine != nil { return }
    guard freigabe() else { emit(["error": "mic_denied", "mic": AVCaptureDevice.authorizationStatus(for: .audio).rawValue]); return }
    let e = AVAudioEngine()
    let input = e.inputNode
    let quelle = input.outputFormat(forBus: 0)
    guard quelle.sampleRate > 0, let wandler = AVAudioConverter(from: quelle, to: ziel) else {
        emit(["error": "engine", "detail": "kein Eingang"]); return
    }
    do {
        datei = try AVAudioFile(forWriting: URL(fileURLWithPath: pfad), settings: [
            AVFormatIDKey: kAudioFormatMPEG4AAC, AVSampleRateKey: 16000, AVNumberOfChannelsKey: 1, AVEncoderBitRateKey: 32000,
        ], commonFormat: .pcmFormatFloat32, interleaved: false)
    } catch { emit(["error": "file", "detail": "\(error)"]); return }
    dateiPfad = pfad; frames = 0
    input.installTap(onBus: 0, bufferSize: 1024, format: quelle) { buf, _ in
        let kap = AVAudioFrameCount(Double(buf.frameLength) * 16000 / quelle.sampleRate) + 32
        guard let aus = AVAudioPCMBuffer(pcmFormat: ziel, frameCapacity: kap) else { return }
        var gegeben = false
        var fehler: NSError?
        wandler.convert(to: aus, error: &fehler) { _, status in
            if gegeben { status.pointee = .noDataNow; return nil }
            gegeben = true; status.pointee = .haveData; return buf
        }
        let n = Int(aus.frameLength)
        if n == 0 || fehler != nil { return }
        schreiben.async {
            try? datei?.write(from: aus)
            frames += n
            let f = aus.floatChannelData![0]
            var pcm = Data(count: n * 2)
            var summe: Float = 0
            pcm.withUnsafeMutableBytes { (p: UnsafeMutableRawBufferPointer) in
                let z = p.bindMemory(to: Int16.self)
                for i in 0..<n {
                    let v = max(-1, min(1, f[i])); summe += v * v
                    z[i] = Int16(v * 32767).littleEndian
                }
            }
            let db = 10 * log10(summe / Float(n) + 1e-12)
            emit(["pcm": pcm.base64EncodedString(), "level": Double(max(0, min(1, (db + 60) / 60)))])
        }
    }
    e.prepare()
    do { try e.start() } catch {
        input.removeTap(onBus: 0); datei = nil; try? FileManager.default.removeItem(atPath: pfad)
        emit(["error": "engine", "detail": "\(error)"]); return
    }
    engine = e
    emit(["state": "recording", "device": geraet(), "rate": quelle.sampleRate])
}

func beenden(behalten: Bool) {
    guard let e = engine else { emit(["state": "stopped", "kept": false, "frames": 0]); return }
    e.inputNode.removeTap(onBus: 0)
    e.stop()
    engine = nil
    // Alles schon Konvertierte ist geschrieben/gesendet, DANN schliesst die Datei.
    schreiben.sync {
        datei = nil
        if !behalten || frames == 0 { try? FileManager.default.removeItem(atPath: dateiPfad) }
        emit(["state": "stopped", "kept": behalten && frames > 0, "frames": frames])
    }
}

emit(["ready": true, "mic": AVCaptureDevice.authorizationStatus(for: .audio).rawValue, "device": geraet()])
while let zeile = readLine(strippingNewline: true) {
    let z = zeile.trimmingCharacters(in: .whitespaces)
    if z.hasPrefix("start ") { starten(String(z.dropFirst(6))) }
    else if z == "stop" { beenden(behalten: true) }
    else if z == "cancel" { beenden(behalten: false) }
    else if z == "status" { emit(["ready": true, "mic": AVCaptureDevice.authorizationStatus(for: .audio).rawValue, "device": geraet()]) }
    else if z == "quit" { break }
}
// stdin zu (Noki beendet): Mikrofon sicher schliessen.
if engine != nil { beenden(behalten: false) }
exit(0)
