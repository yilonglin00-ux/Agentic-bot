// Noki Photos Adapter: sichere macOS-Schnittstelle zur Apple-Fotomediathek via PhotoKit
// Gibt Metadaten und Thumbnails zurück, ohne jemals intern in .photoslibrary herumzulesen.
// Befehle:
//   noki-photos status
//   noki-photos request
//   noki-photos list [offset] [limit]
//   noki-photos thumb <asset_id>
//   noki-photos export <asset_id> <target_path>

import Foundation
import Photos
import AppKit

setvbuf(stdout, nil, _IOLBF, 0)

func logMsg(_ s: String) {
    fputs(s + "\n", stderr)
    let logPath = "/Users/yilonglin/NOKI/desktop/noki_live.log"
    if let data = (s + "\n").data(using: .utf8) {
        if let handle = try? FileHandle(forWritingTo: URL(fileURLWithPath: logPath)) {
            handle.seekToEndOfFile()
            handle.write(data)
            try? handle.close()
        }
    }
}

func statusString(_ s: PHAuthorizationStatus) -> String {
    switch s {
    case .notDetermined: return "not_determined"
    case .restricted:    return "restricted"
    case .denied:        return "denied"
    case .authorized:    return "authorized"
    case .limited:       return "limited"
    @unknown default:    return "unknown"
    }
}

func currentStatus() -> PHAuthorizationStatus {
    if #available(macOS 11.0, *) {
        return PHPhotoLibrary.authorizationStatus(for: .readWrite)
    } else {
        return PHPhotoLibrary.authorizationStatus()
    }
}

func outputJson(_ obj: Any) {
    if let data = try? JSONSerialization.data(withJSONObject: obj, options: []),
       let str = String(data: data, encoding: .utf8) {
        print(str)
    } else {
        print("{}")
    }
}

func requestAuthIfNeeded() -> PHAuthorizationStatus {
    var st = currentStatus()
    if st == .notDetermined {
        logMsg("[PHOTOS_HELPER] status is not_determined, requesting authorization...")
        let sem = DispatchSemaphore(value: 0)
        if #available(macOS 11.0, *) {
            PHPhotoLibrary.requestAuthorization(for: .readWrite) { s in
                st = s
                sem.signal()
            }
        } else {
            PHPhotoLibrary.requestAuthorization { s in
                st = s
                sem.signal()
            }
        }
        let start = Date()
        while sem.wait(timeout: .now() + 0.1) == .timedOut {
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
            if Date().timeIntervalSince(start) > 8.0 { break }
        }
        logMsg("[PHOTOS_HELPER] after request authorization status=\(statusString(st))")
    }
    return st
}

let args = CommandLine.arguments
let cmd = args.count > 1 ? args[1] : "status"

switch cmd {
case "status":
    let st = currentStatus()
    outputJson([
        "ok": true,
        "status": statusString(st),
        "code": st.rawValue
    ])

case "request":
    let st = requestAuthIfNeeded()
    outputJson([
        "ok": true,
        "status": statusString(st),
        "code": st.rawValue
    ])

case "list":
    let st = requestAuthIfNeeded()
    guard st == .authorized || st == .limited else {
        let hinweis: String
        switch st {
        case .denied, .restricted:
            hinweis = "Zugriff auf Fotos nicht erlaubt. In macOS Systemeinstellungen > Datenschutz & Sicherheit freigeben."
        default:
            hinweis = "Fotos-Berechtigung erforderlich. Bitte Zugriff in macOS Systemeinstellungen freigeben."
        }
        logMsg("[PHOTOS_HELPER] not authorized: \(statusString(st))")
        outputJson([
            "ok": false,
            "status": statusString(st),
            "code": st.rawValue,
            "error": "not_authorized",
            "hinweis": hinweis,
            "assets": []
        ])
        exit(0)
    }

    let offset = max(0, args.count > 2 ? (Int(args[2]) ?? 0) : 0)
    let limit = max(1, min(args.count > 3 ? (Int(args[3]) ?? 40) : 40, 100))
    let results = PHAsset.fetchAssets(with: .image, options: nil)
    logMsg("[PHOTOS_HELPER] auth=\(statusString(st)) assets_fetched=\(results.count)")

    var items: [[String: Any]] = []
    let isoFmt = ISO8601DateFormatter()
    var ordered: [PHAsset] = []
    ordered.reserveCapacity(results.count)
    results.enumerateObjects { asset, _, _ in ordered.append(asset) }
    ordered.sort {
        let a = $0.creationDate ?? $0.modificationDate ?? .distantPast
        let b = $1.creationDate ?? $1.modificationDate ?? .distantPast
        return a == b ? $0.localIdentifier < $1.localIdentifier : a > b
    }
    let start = min(offset, ordered.count)
    let end = min(start + limit, ordered.count)
    for idx in start..<end {
        let asset = ordered[idx]
        let resources = PHAssetResource.assetResources(for: asset)
        let filename = resources.first?.originalFilename ?? "Foto_\(idx + 1).jpg"
        let dateStr = (asset.creationDate ?? asset.modificationDate).map { isoFmt.string(from: $0) } ?? ""
        items.append([
            "id": asset.localIdentifier,
            "name": filename,
            "width": asset.pixelWidth,
            "height": asset.pixelHeight,
            "created": dateStr,
            "favorite": asset.isFavorite
        ])
    }
    logMsg("[PHOTOS_HELPER] assets_serialized=\(items.count)")

    outputJson([
        "ok": true,
        "status": statusString(st),
        "count": ordered.count,
        "offset": start,
        "assets": items
    ])

case "thumb":
    guard args.count > 2 else {
        logMsg("[PHOTOS_HELPER] thumb: Fehlende Asset-ID")
        exit(1)
    }
    let assetId = args[2]
    let st = currentStatus()
    guard st == .authorized || st == .limited else {
        logMsg("[PHOTOS_HELPER] thumb: Nicht berechtigt \(statusString(st))")
        exit(2)
    }

    let fetchRes = PHAsset.fetchAssets(withLocalIdentifiers: [assetId], options: nil)
    guard let asset = fetchRes.firstObject else {
        logMsg("[PHOTOS_HELPER] thumb: Asset nicht gefunden \(assetId)")
        exit(3)
    }

    let manager = PHImageManager.default()
    let opt = PHImageRequestOptions()
    opt.isSynchronous = true
    opt.deliveryMode = .fastFormat
    opt.resizeMode = .fast
    opt.isNetworkAccessAllowed = true

    var base64Data = ""
    let targetSize = CGSize(width: 260, height: 260)
    manager.requestImage(for: asset, targetSize: targetSize, contentMode: .aspectFill, options: opt) { img, _ in
        guard let img = img else { return }
        if let tiff = img.tiffRepresentation,
           let bitmap = NSBitmapImageRep(data: tiff),
           let jpeg = bitmap.representation(using: .jpeg, properties: [.compressionFactor: 0.80]) {
            base64Data = "data:image/jpeg;base64," + jpeg.base64EncodedString()
        }
    }

    if base64Data.isEmpty {
        logMsg("[PHOTOS_HELPER] thumb: Thumbnail konnte nicht gerendert werden: \(assetId)")
        exit(4)
    }
    logMsg("[PHOTOS_HELPER] thumb_success: \(assetId) len=\(base64Data.count)")
    print(base64Data)

case "export":
    guard args.count > 3 else {
        outputJson(["ok": false, "error": "Fehlende Argumente: asset_id und target_path nötig"])
        exit(1)
    }
    let assetId = args[2]
    let targetPath = args[3]
    let st = currentStatus()
    guard st == .authorized || st == .limited else {
        outputJson(["ok": false, "error": "Nicht berechtigt", "status": statusString(st)])
        exit(2)
    }

    let fetchRes = PHAsset.fetchAssets(withLocalIdentifiers: [assetId], options: nil)
    guard let asset = fetchRes.firstObject else {
        outputJson(["ok": false, "error": "Asset nicht gefunden"])
        exit(3)
    }

    let targetUrl = URL(fileURLWithPath: targetPath)
    let parentDir = targetUrl.deletingLastPathComponent()
    try? FileManager.default.createDirectory(at: parentDir, withIntermediateDirectories: true)

    let resources = PHAssetResource.assetResources(for: asset)
    if let primaryRes = resources.first(where: { $0.type == .photo || $0.type == .fullSizePhoto }) ?? resources.first {
        let opt = PHAssetResourceRequestOptions()
        opt.isNetworkAccessAllowed = true
        let sem = DispatchSemaphore(value: 0)
        var writeErr: Error? = nil

        // Falls targetPath bereits existiert, vorher entfernen
        try? FileManager.default.removeItem(at: targetUrl)

        PHAssetResourceManager.default().writeData(for: primaryRes, toFile: targetUrl, options: opt) { err in
            writeErr = err
            sem.signal()
        }
        _ = sem.wait(timeout: .now() + 15.0)

        if writeErr == nil && FileManager.default.fileExists(atPath: targetPath) {
            let sz = (try? FileManager.default.attributesOfItem(atPath: targetPath)[.size] as? UInt64) ?? 0
            outputJson(["ok": true, "path": targetPath, "groesse": sz])
            exit(0)
        }
    }

    // Fallback über PHImageManager requestImageDataAndOrientation
    let opt = PHImageRequestOptions()
    opt.isSynchronous = true
    opt.deliveryMode = .highQualityFormat
    opt.isNetworkAccessAllowed = true

    var success = false
    var fileSize: UInt64 = 0
    if #available(macOS 10.15, *) {
        PHImageManager.default().requestImageDataAndOrientation(for: asset, options: opt) { data, uti, _, _ in
            guard let data = data else { return }
            do {
                try data.write(to: targetUrl)
                success = true
                fileSize = UInt64(data.count)
            } catch {}
        }
    }

    if success {
        outputJson(["ok": true, "path": targetPath, "groesse": fileSize])
    } else {
        outputJson(["ok": false, "error": "Export fehlgeschlagen"])
        exit(4)
    }

default:
    fputs("Unbekannter Befehl: \(cmd)\n", stderr)
    exit(1)
}
