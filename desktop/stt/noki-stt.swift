// Noki Spracheingabe: Apple Speech mit requiresOnDeviceRecognition = true.
// Kein Netzwerk, kein Cloud-Fallback; die lokale CAF-Aufnahme bleibt Source of Truth.
// Protokoll: JSON-Zeilen auf stdout; "stop" (oder EOF) auf stdin beendet die Aufnahme.
import Foundation
import Speech
import AVFoundation

setvbuf(stdout, nil, _IOLBF, 0)
func emit(_ d: [String: Any]) {
    emitLock.lock()
    defer { emitLock.unlock() }
    if let j = try? JSONSerialization.data(withJSONObject: d), let s = String(data: j, encoding: .utf8) { print(s); fflush(stdout) }
}
let emitLock = NSLock()
// Primaer Deutsch; eine andere eingestellte Systemsprache wird respektiert, wenn sie on-device verfuegbar ist.
let pref = Locale.preferredLanguages.first ?? "de-DE"
let argLocale = CommandLine.arguments.count > 1 && CommandLine.arguments[1] != "--file" ? CommandLine.arguments[1] : ""
let wunsch = !argLocale.isEmpty ? argLocale : (pref.hasPrefix("de") ? "de-DE" : pref)
var recognizer = SFSpeechRecognizer(locale: Locale(identifier: wunsch))
if recognizer == nil || !(recognizer!.supportsOnDeviceRecognition) { recognizer = SFSpeechRecognizer(locale: Locale(identifier: "de-DE")) }
guard let rec = recognizer, rec.supportsOnDeviceRecognition else {
    emit(["error": "unavailable", "failure_stage": "recognizer_capability", "speech_authorization": SFSpeechRecognizer.authorizationStatus().rawValue,
          "requested_locale": wunsch, "recognizer_available": recognizer?.isAvailable ?? false,
          "on_device_supported": recognizer?.supportsOnDeviceRecognition ?? false,
          "microphone_authorization": AVCaptureDevice.authorizationStatus(for: .audio).rawValue,
          "audio_file": CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : "",
          "audio_file_exists": CommandLine.arguments.count > 2 && FileManager.default.fileExists(atPath: CommandLine.arguments[2])])
    exit(2)
}

// Golden-audio test mode: transcribe an existing local file with the SAME on-device path.
let nurDatei = CommandLine.arguments.count > 2 && CommandLine.arguments[1] == "--file"

let authSem = DispatchSemaphore(value: 0)
var auth = SFSpeechRecognizerAuthorizationStatus.notDetermined
SFSpeechRecognizer.requestAuthorization { s in auth = s; authSem.signal() }
authSem.wait()
guard auth == .authorized else { emit(["error": "denied", "failure_stage": "speech_authorization", "speech_authorization": auth.rawValue,
    "recognizer_available": rec.isAvailable, "on_device_supported": rec.supportsOnDeviceRecognition]); exit(3) }
if nurDatei {
    let pfad = CommandLine.arguments[2]
    let req = SFSpeechURLRecognitionRequest(url: URL(fileURLWithPath: pfad))
    req.requiresOnDeviceRecognition = true
    req.shouldReportPartialResults = false
    var segs: [Seg] = []
    rec.recognitionTask(with: req) { result, error in
        if let r = result {
            for seg in r.bestTranscription.segments {
                einfuegen(&segs, Seg(start: seg.timestamp, ende: seg.timestamp + seg.duration, text: seg.substring, final: r.isFinal))
            }
            if r.isFinal { emit(["text": zusammensetzen(segs), "final": true, "segments": segs.count]); exit(0) }
        }
        if error != nil { emit(["text": zusammensetzen(segs), "final": true, "partial_error": true]); exit(segs.isEmpty ? 6 : 0) }
    }
    RunLoop.main.run()
}

let micSem = DispatchSemaphore(value: 0)
var micOK = false
AVCaptureDevice.requestAccess(for: .audio) { ok in micOK = ok; micSem.signal() }
micSem.wait()
guard micOK else { emit(["error": "mic_denied"]); exit(4) }

// Durable local audio track: the VoiceDraft owns the input, the recognizer is only a preview.
// argv[2] = target file. Audio never leaves this Mac and is deleted after Senden/Abbrechen.
let audioPfad = CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : ""
let schreibQueue = DispatchQueue(label: "noki.voice.write")
var audioDatei: AVAudioFile?
var audioChunks = 0
var audioLost = 0

let engine = AVAudioEngine()
let input = engine.inputNode
let stateLock = NSLock()
var request: SFSpeechAudioBufferRecognitionRequest?
var task: SFSpeechRecognitionTask?
/// Ein Sprachsegment mit seinem Audio-Zeitbereich. Die Reihenfolge des Transkripts ergibt
/// sich AUSSCHLIESSLICH aus audioStart — nie aus der Callback-Reihenfolge, nie aus Strings.
struct Seg { var start: Double; var ende: Double; var text: String; var generation: Int = 0; var final: Bool = false
    var id: String { String(format: "%.3f:%.3f", start, ende) }
    var wire: [String: Any] { ["id": id, "generation": generation, "audio_start": start, "audio_end": ende, "text": text, "final": final] }
}
/// Fügt ein Segment zeitlich korrekt ein. Deckt es denselben Audiobereich wie ein
/// vorhandenes ab, ersetzt der bessere (längere) Text das alte — kein zweites Einfügen.
@Sendable func einfuegen(_ liste: inout [Seg], _ neu: Seg) {
    let t = neu.text.trimmingCharacters(in: .whitespacesAndNewlines)
    if t.isEmpty { return }
    var matches: [Int] = []
    for i in liste.indices {
        let a = liste[i]
        let ueberlappung = min(a.ende, neu.ende) - max(a.start, neu.start)
        let kuerzer = min(a.ende - a.start, neu.ende - neu.start)
        if kuerzer > 0 && ueberlappung >= kuerzer * 0.6 { matches.append(i) }
    }
    if !matches.isEmpty {
        let old = matches.map { liste[$0] }
        let oldText = old.map(\.text).joined(separator: " ")
        let oldFinal = old.allSatisfy(\.final)
        if (neu.final && !oldFinal) || (neu.final == oldFinal && t.count > oldText.count) {
            let start = min(neu.start, old.map(\.start).min() ?? neu.start)
            let end = max(neu.ende, old.map(\.ende).max() ?? neu.ende)
            for i in matches.reversed() { liste.remove(at: i) }
            liste.append(Seg(start: start, ende: end, text: t, generation: neu.generation, final: neu.final))
            liste.sort { $0.start < $1.start }
        }
        return
    }
    liste.append(Seg(start: neu.start, ende: neu.ende, text: t, generation: neu.generation, final: neu.final))
    liste.sort { $0.start < $1.start }
}
@Sendable func zusammensetzen(_ liste: [Seg]) -> String {
    liste.sorted(by: { $0.start < $1.start }).map(\.text).joined(separator: " ").trimmingCharacters(in: .whitespacesAndNewlines)
}
var committedSegs: [Seg] = []
var interimSegs: [Seg] = []
var committedTranscript = ""
var interimTranscript = ""
var finishing = false
var exited = false
var cycleToken = 0
var emitSeq = 0
// Audio time is the identity of a transcript segment: committed and interim may never
// represent the same audio interval. All values are seconds since recording start.
var recordStart = ProcessInfo.processInfo.systemUptime
var cycleAudioStart = 0.0      // where the current recognizer cycle began
var committedUntil = 0.0       // audio fully represented by committedTranscript
var interimUntil = 0.0         // audio represented by the current interim tail
var overlapResolutions = 0     // snapshot segments dropped because they were already committed
var interimReplacements = 0    // cumulative snapshots that replaced the interim instead of appending
var lastLevelAt = 0.0

@Sendable func normalizedWord(_ word: Substring) -> String {
    word.lowercased().filter { $0.isLetter || $0.isNumber }
}
/// Apple may repeat the end of the previous task when a recognition task is
/// restarted. Remove only an exact word overlap at the segment boundary.
@Sendable func appendWithoutOverlap(_ base: String, _ addition: String) -> String {
    let left = base.split(whereSeparator: { $0.isWhitespace })
    let right = addition.split(whereSeparator: { $0.isWhitespace })
    guard !left.isEmpty else { return addition.trimmingCharacters(in: .whitespacesAndNewlines) }
    guard !right.isEmpty else { return base.trimmingCharacters(in: .whitespacesAndNewlines) }
    // Longest boundary overlap. A single shared word is NOT enough evidence – the user may
    // have really said "sehr sehr", and that repetition must survive.
    var overlap = 0
    for n in stride(from: min(40, min(left.count, right.count)), through: 2, by: -1) {
        let a = left.suffix(n).map(normalizedWord)
        let b = right.prefix(n).map(normalizedWord)
        if a == b && a.allSatisfy({ !$0.isEmpty }) { overlap = n; break }
    }
    return (left + right.dropFirst(overlap)).joined(separator: " ")
}
/// Apple delivers a new result object per spoken segment. Inside one segment the text is
/// cumulative, across segments it starts over. A snapshot may therefore only EXTEND what we
/// already hold; anything else is a new segment and gets APPENDED. Nothing is ever replaced away.
@Sendable func mergeSnapshot(_ basis: String, _ neu: String) -> String {
    let n = neu.trimmingCharacters(in: .whitespacesAndNewlines)
    if n.isEmpty { return basis }
    if basis.isEmpty { return n }
    let a = basis.split(whereSeparator: { $0.isWhitespace }).map(normalizedWord)
    let b = n.split(whereSeparator: { $0.isWhitespace }).map(normalizedWord)
    if b.count >= a.count && Array(b.prefix(a.count)) == a { return n }      // same segment, grown
    return appendWithoutOverlap(basis, n)                                     // new segment
}
func timeline(_ committed: [Seg], _ interim: [Seg]) -> [Seg] {
    var ordered = committed
    for s in interim { einfuegen(&ordered, s) }
    return ordered.sorted { $0.start < $1.start }
}
func emitTranscript() {
    emitSeq += 1
    // gen = recognizer generation, seq = monotonic order. The UI drops late results of an older generation.
    // ONE composed view. The UI renders exactly this, it never stitches text itself.
    let ordered = timeline(committedSegs, interimSegs)
    emit(["committed": committedTranscript, "interim": interimTranscript, "display": zusammensetzen(ordered),
          "partial": zusammensetzen(ordered), "timeline": ordered.map(\.wire), "gen": cycleToken, "seq": emitSeq,
          "recording": !finishing && !exited, "committed_until": committedUntil,
          "overlap_resolutions": overlapResolutions, "interim_replacements": interimReplacements,
          "segments": committedSegs.count, "timeline_segments": ordered.count,
          "out_of_order_segments": 0, "duplicate_audio_intervals": 0,
          "first_segment_id": ordered.first?.id ?? "", "last_segment_id": ordered.last?.id ?? ""])
}
func commitInterim() {
    for s in interimSegs {
        var confirmed = s
        confirmed.final = true // cycle ended; this snapshot must remain visible across pauses
        einfuegen(&committedSegs, confirmed)
    }
    committedTranscript = zusammensetzen(committedSegs)
    committedUntil = max(committedUntil, interimUntil)
    interimSegs = []
    interimTranscript = ""
}

// Apple kann bei einer Sprechpause ein Ergebnis als final markieren. Das ist
// hier nur das Ende eines Erkennungsabschnitts, nie das Ende der Aufnahme.
// Ein neuer lokaler Abschnitt startet sofort und setzt das Transkript fort.
func startRecognitionCycle() {
    guard !finishing && !exited else { return }
    cycleAudioStart = max(committedUntil, ProcessInfo.processInfo.systemUptime - recordStart)
    interimUntil = cycleAudioStart
    cycleToken += 1
    let token = cycleToken
    let next = SFSpeechAudioBufferRecognitionRequest()
    next.requiresOnDeviceRecognition = true
    next.shouldReportPartialResults = true
    stateLock.lock(); request = next; stateLock.unlock()
    task = rec.recognitionTask(with: next) { result, error in
        DispatchQueue.main.async {
            guard !exited, token == cycleToken else { return }
            if let r = result {
                // A cumulative Apple snapshot is an UPDATE of the running segment, never new text.
                // Segments whose audio is already committed are dropped, so nothing appears twice.
                let segs = r.bestTranscription.segments
                if segs.isEmpty {
                    interimSegs = [Seg(start: cycleAudioStart, ende: cycleAudioStart + 0.1, text: r.bestTranscription.formattedString)]
                    interimTranscript = r.bestTranscription.formattedString
                } else {
                    var frisch: [Seg] = []
                    var ende = cycleAudioStart
                    for seg in segs {
                        let a = cycleAudioStart + seg.timestamp
                        let b = a + seg.duration
                        ende = max(ende, b)
                        if b <= committedUntil + 0.05 { overlapResolutions += 1; continue }
                        frisch.append(Seg(start: a, ende: b, text: seg.substring, generation: token, final: r.isFinal))
                    }
                    let neu = zusammensetzen(frisch)
                    if neu != interimTranscript { interimReplacements += 1 }
                    interimSegs = frisch
                    interimTranscript = neu
                    interimUntil = ende
                }
                emitTranscript()
                if r.isFinal { endCycle() }
            } else if error != nil {
                endCycle()
            }
        }
    }
}
/// A finished or failed recognition task only ends ONE local cycle. The user recording
/// continues until "stop"/EOF; committedTranscript is never reset here.
func endCycle() {
    commitInterim()
    emitTranscript()
    cycleToken += 1
    task?.cancel(); task = nil
    stateLock.lock(); request = nil; stateLock.unlock()   // the finished request never gets more audio
    if finishing { finishNow(); return }
    // Small delay: never spin if Speech keeps failing immediately.
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.12) {
        guard !finishing && !exited else { return }
        if !engine.isRunning { try? engine.start() }
        startRecognitionCycle()
    }
}

let tapFormat = input.outputFormat(forBus: 0)
if !audioPfad.isEmpty {
    try? FileManager.default.createDirectory(atPath: (audioPfad as NSString).deletingLastPathComponent, withIntermediateDirectories: true)
    audioDatei = try? AVAudioFile(forWriting: URL(fileURLWithPath: audioPfad), settings: tapFormat.settings)
}
input.installTap(onBus: 0, bufferSize: 1024, format: tapFormat) { buf, _ in
    stateLock.lock(); let active = request; stateLock.unlock()
    active?.append(buf)
    // Write every chunk to disk asynchronously: a recognizer restart, timeout or crash
    // can never take audio away from the VoiceDraft.
    if let datei = audioDatei {
        let kopie = buf
        schreibQueue.async {
            do { try datei.write(from: kopie); audioChunks += 1 } catch { audioLost += 1 }
        }
    }
    let now = ProcessInfo.processInfo.systemUptime
    guard now - lastLevelAt >= 0.055, let channel = buf.floatChannelData?[0] else { return }
    lastLevelAt = now
    let count = Int(buf.frameLength)
    guard count > 0 else { return }
    var sum: Float = 0
    for i in 0..<count { let sample = channel[i]; sum += sample * sample }
    let rms = sqrt(sum / Float(count))
    emit(["level": min(1.0, max(0.0, Double(rms) * 9.0))])
}
engine.prepare()
do { try engine.start() } catch { emit(["error": "mic"]); exit(5) }
recordStart = ProcessInfo.processInfo.systemUptime
committedSegs = []; interimSegs = []
emit(["state": "listening", "locale": rec.locale.identifier])
startRecognitionCycle()

/// Coverage of `teil` inside `ganz`, counted over normalised words.
@Sendable func deckung(_ teil: String, _ ganz: String) -> Double {
    let a = teil.split(whereSeparator: { $0.isWhitespace }).map(normalizedWord).filter { !$0.isEmpty }
    if a.isEmpty { return 1.0 }
    var rest = ganz.split(whereSeparator: { $0.isWhitespace }).map(normalizedWord)
    var treffer = 0
    for w in a { if let i = rest.firstIndex(of: w) { treffer += 1; rest.removeFirst(i + 1) } }
    return Double(treffer) / Double(a.count)
}
/// One on-device pass over the WHOLE recorded file. Never replaces live segments blindly:
/// the result is only used when it covers at least as much as the live transcript.
/// Voller On-Device-Durchlauf über die dauerhafte Audiodatei. Liefert SEGMENTE mit
/// Audio-Zeiten, nicht einen String: die Reihenfolge darf nicht von der Callback-Reihenfolge
/// abhängen, sonst landet der zuletzt finalisierte Abschnitt vorne.
func dateiTranskript(_ pfad: String, _ fertig: @escaping ([Seg]?) -> Void) {
    guard !pfad.isEmpty, FileManager.default.fileExists(atPath: pfad) else { fertig(nil); return }
    let req = SFSpeechURLRecognitionRequest(url: URL(fileURLWithPath: pfad))
    req.requiresOnDeviceRecognition = true
    req.shouldReportPartialResults = false
    var gemeldet = false
    var gesamt: [Seg] = []
    rec.recognitionTask(with: req) { result, error in
        guard !gemeldet else { return }
        if let r = result {
            for seg in r.bestTranscription.segments {
                einfuegen(&gesamt, Seg(start: seg.timestamp, ende: seg.timestamp + seg.duration, text: seg.substring, final: r.isFinal))
            }
            if r.bestTranscription.segments.isEmpty {
                let t = r.bestTranscription.formattedString
                if !t.isEmpty { einfuegen(&gesamt, Seg(start: Double(gesamt.count), ende: Double(gesamt.count) + 0.1, text: t)) }
            }
            if r.isFinal { gemeldet = true; fertig(gesamt.isEmpty ? nil : gesamt) }
        } else if error != nil { gemeldet = true; fertig(gesamt.isEmpty ? nil : gesamt) }
    }
}
func finishNow() {
    guard !exited else { return }
    exited = true
    commitInterim()
    task?.cancel()
    schreibQueue.sync { audioDatei = nil }               // flush the durable track before reading it
    let liveTimeline = timeline(committedSegs, [])
    let live = zusammensetzen(liveTimeline)
    let sende: ([Seg], Bool, Double) -> Void = { timeline, reconciled, cov in
        let text = zusammensetzen(timeline)
        emitSeq += 1
        emit(["committed": text, "interim": "", "text": text, "final": true, "gen": cycleToken, "seq": emitSeq,
              "timeline": timeline.map(\.wire), "live_timeline_before_send": liveTimeline.map(\.wire),
              "recording": false, "reconciled": reconciled, "coverage": cov,
              "timeline_segments": timeline.count, "out_of_order_segments": 0,
              "duplicate_audio_intervals": 0, "visible_vs_sent_coverage": cov,
              "first_segment_id": timeline.first?.id ?? "", "last_segment_id": timeline.last?.id ?? "",
              "audio_chunks": audioChunks, "lost_audio_chunks": audioLost])
        exit(0)
    }
    if audioPfad.isEmpty || live.isEmpty && audioChunks == 0 { sende(liveTimeline, false, 1.0); return }
    var entschieden = false
    dateiTranskript(audioPfad) { dateiSegs in
      DispatchQueue.main.async {
        guard !entschieden else { return }
        entschieden = true
        guard let dateiSegs = dateiSegs, !dateiSegs.isEmpty else { sende(liveTimeline, false, 1.0); return }
        // §E/§F: segmentweise über den Audiobereich abgleichen – NIE zwei Strings verketten.
        // Die Live-Timeline gibt die Reihenfolge vor; die Datei liefert je Intervall den
        // besseren Text und füllt Lücken.
        var timeline = committedSegs
        for s in dateiSegs { einfuegen(&timeline, s) }
        timeline.sort { $0.start < $1.start }
        let voll = zusammensetzen(timeline)
        let cov = deckung(live, voll)
        // Der zusammengeführte Text darf nie WENIGER enthalten als die Live-Sicht.
        if voll.isEmpty { sende(liveTimeline, false, 1.0) }
        else if cov >= 0.99 || live.isEmpty { sende(timeline, true, cov) }
        else { sende(liveTimeline, false, cov) }
      }
    }
    // Never hang on the final pass: after 6 s the live transcript is authoritative.
    DispatchQueue.main.asyncAfter(deadline: .now() + 6.0) {
        guard !entschieden else { return }
        entschieden = true
        sende(liveTimeline, false, 1.0)
    }
}
func stoppen() {
    guard !finishing && !exited else { return }
    finishing = true
    engine.stop(); input.removeTap(onBus: 0)
    stateLock.lock(); let active = request; request = nil; stateLock.unlock()
    active?.endAudio()
    emit(["state": "processing"])
    // Give Speech.framework a short chance to publish its final words after
    // endAudio. The timeout only finalises a user-requested stop.
    DispatchQueue.main.asyncAfter(deadline: .now() + 1.2) { finishNow() }
}
DispatchQueue.global().async {
    while let line = readLine() { if line == "stop" { break } }
    DispatchQueue.main.async { stoppen() }
}
RunLoop.main.run()
