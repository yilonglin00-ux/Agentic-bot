#import <Foundation/Foundation.h>
#import <Photos/Photos.h>
#import <AppKit/AppKit.h>

int noki_photos_status(void) {
    if (@available(macOS 11.0, *)) {
        return (int)[PHPhotoLibrary authorizationStatusForAccessLevel:PHAccessLevelReadWrite];
    } else {
        return (int)[PHPhotoLibrary authorizationStatus];
    }
}

int noki_photos_request(void) {
    int st = noki_photos_status();
    if (st == 0) { // notDetermined
        __block int new_st = 0;
        dispatch_semaphore_t sem = dispatch_semaphore_create(0);
        if (@available(macOS 11.0, *)) {
            [PHPhotoLibrary requestAuthorizationForAccessLevel:PHAccessLevelReadWrite handler:^(PHAuthorizationStatus status) {
                new_st = (int)status;
                dispatch_semaphore_signal(sem);
            }];
        } else {
            [PHPhotoLibrary requestAuthorization:^(PHAuthorizationStatus status) {
                new_st = (int)status;
                dispatch_semaphore_signal(sem);
            }];
        }
        NSDate *start = [NSDate date];
        while (dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 100 * NSEC_PER_MSEC)) != 0) {
            [[NSRunLoop currentRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.1]];
            if ([[NSDate date] timeIntervalSinceDate:start] > 8.0) break;
        }
        st = (new_st != 0) ? new_st : noki_photos_status();
    }
    return st;
}

char* noki_photos_list(int offset, int limit) {
    int st = noki_photos_request();
    if (st != 3 && st != 4) { // not authorized and not limited
        NSString *err = (st == 2 || st == 1) 
            ? @"Zugriff auf Fotos nicht erlaubt. In macOS Systemeinstellungen > Datenschutz & Sicherheit freigeben."
            : @"Fotos-Berechtigung erforderlich. Bitte Zugriff in macOS Systemeinstellungen freigeben.";
        NSString *json = [NSString stringWithFormat:@"{\"ok\":false,\"status\":\"%@\",\"code\":%d,\"hinweis\":\"%@\",\"assets\":[]}", 
            st == 2 ? @"denied" : (st == 1 ? @"restricted" : @"not_determined"), st, err];
        return strdup([json UTF8String]);
    }

    // Jeder Aufruf liest PhotoKit neu. Es gibt keine gecachte Liste und kein
    // fetchLimit: offset/limit begrenzen nur die Metadaten einer UI-Seite.
    PHFetchResult<PHAsset *> *results = [PHAsset fetchAssetsWithMediaType:PHAssetMediaTypeImage options:nil];
    NSMutableArray<PHAsset *> *ordered = [NSMutableArray arrayWithCapacity:results.count];
    for (PHAsset *asset in results) [ordered addObject:asset];
    [ordered sortUsingComparator:^NSComparisonResult(PHAsset *a, PHAsset *b) {
        NSDate *ad = a.creationDate ?: a.modificationDate ?: [NSDate distantPast];
        NSDate *bd = b.creationDate ?: b.modificationDate ?: [NSDate distantPast];
        NSComparisonResult cmp = [bd compare:ad];
        return cmp == NSOrderedSame ? [a.localIdentifier compare:b.localIdentifier] : cmp;
    }];
    NSUInteger start = MIN((NSUInteger)MAX(offset, 0), ordered.count);
    NSUInteger pageSize = (NSUInteger)(limit > 0 ? MIN(limit, 100) : 40);
    NSUInteger end = MIN(start + pageSize, ordered.count);
    NSMutableArray *items = [NSMutableArray arrayWithCapacity:end - start];
    NSISO8601DateFormatter *isoFmt = [[NSISO8601DateFormatter alloc] init];

    for (NSUInteger i = start; i < end; i++) {
        PHAsset *asset = ordered[i];
        NSArray<PHAssetResource *> *resources = [PHAssetResource assetResourcesForAsset:asset];
        NSString *filename = resources.firstObject.originalFilename ?: [NSString stringWithFormat:@"Foto_%lu.jpg", (unsigned long)(i + 1)];
        NSDate *effectiveDate = asset.creationDate ?: asset.modificationDate;
        NSString *dateStr = effectiveDate ? [isoFmt stringFromDate:effectiveDate] : @"";
        [items addObject:@{
            @"id": asset.localIdentifier ?: @"",
            @"name": filename,
            @"width": @(asset.pixelWidth),
            @"height": @(asset.pixelHeight),
            @"created": dateStr,
            @"favorite": @(asset.isFavorite)
        }];
    }

    NSDictionary *dict = @{
        @"ok": @YES,
        @"status": st == 3 ? @"authorized" : @"limited",
        @"count": @(ordered.count),
        @"offset": @(start),
        @"assets": items
    };

    NSData *data = [NSJSONSerialization dataWithJSONObject:dict options:0 error:nil];
    if (!data) return strdup("{\"ok\":false,\"assets\":[]}");
    NSString *json = [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
    return strdup([json UTF8String]);
}

char* noki_photos_thumb(const char* asset_id, int target_size) {
    if (!asset_id) return NULL;
    NSString *nsId = [NSString stringWithUTF8String:asset_id];
    PHFetchResult<PHAsset *> *res = [PHAsset fetchAssetsWithLocalIdentifiers:@[nsId] options:nil];
    PHAsset *asset = res.firstObject;
    if (!asset) return NULL;

    PHImageManager *manager = [PHImageManager defaultManager];
    PHImageRequestOptions *opt = [[PHImageRequestOptions alloc] init];
    opt.synchronous = YES;
    opt.deliveryMode = PHImageRequestOptionsDeliveryModeHighQualityFormat;
    opt.resizeMode = PHImageRequestOptionsResizeModeExact;
    opt.networkAccessAllowed = YES;

    CGFloat sz = (target_size >= 100 && target_size <= 800) ? (CGFloat)target_size : 360.0;
    __block NSString *dataUri = nil;

    // PhotoKit liefert NSImage-Instanzen teilweise als lazy/degraded backing.
    // Deren CGImageForProposedRect war formal gueltig, enthielt unter paralleler
    // Galerie-Last aber schwarze Pixel. Deshalb immer zuerst die echten Asset-
    // Bytes holen und das Thumbnail rein ueber ImageIO dekodieren.
    [manager requestImageDataAndOrientationForAsset:asset options:opt resultHandler:^(NSData * _Nullable imageData, NSString * _Nullable dataUTI, CGImagePropertyOrientation orientation, NSDictionary * _Nullable info) {
        if (!imageData || imageData.length == 0) return;
        CGImageSourceRef src = CGImageSourceCreateWithData((__bridge CFDataRef)imageData, NULL);
        if (!src) return;
        NSDictionary *thumbOpts = @{
            (id)kCGImageSourceCreateThumbnailWithTransform: @YES,
            (id)kCGImageSourceCreateThumbnailFromImageAlways: @YES,
            (id)kCGImageSourceThumbnailMaxPixelSize: @(sz)
        };
        CGImageRef thumbCg = CGImageSourceCreateThumbnailAtIndex(src, 0, (__bridge CFDictionaryRef)thumbOpts);
        if (thumbCg) {
            NSBitmapImageRep *rep = [[NSBitmapImageRep alloc] initWithCGImage:thumbCg];
            NSData *jpeg = [rep representationUsingType:NSBitmapImageFileTypeJPEG properties:@{NSImageCompressionFactor: @0.82}];
            if (jpeg && jpeg.length > 500) {
                NSString *b64 = [jpeg base64EncodedStringWithOptions:0];
                dataUri = [NSString stringWithFormat:@"data:image/jpeg;base64,%@", b64];
            }
            CGImageRelease(thumbCg);
        }
        CFRelease(src);
    }];

    if (!dataUri) return NULL;
    return strdup([dataUri UTF8String]);
}

int noki_photos_export(const char* asset_id, const char* target_path) {
    if (!asset_id || !target_path) return 0;
    NSString *nsId = [NSString stringWithUTF8String:asset_id];
    NSString *nsTarget = [NSString stringWithUTF8String:target_path];
    PHFetchResult<PHAsset *> *res = [PHAsset fetchAssetsWithLocalIdentifiers:@[nsId] options:nil];
    PHAsset *asset = res.firstObject;
    if (!asset) return 0;

    NSURL *targetUrl = [NSURL fileURLWithPath:nsTarget];
    NSURL *parentDir = [targetUrl URLByDeletingLastPathComponent];
    [[NSFileManager defaultManager] createDirectoryAtURL:parentDir withIntermediateDirectories:YES attributes:nil error:nil];
    [[NSFileManager defaultManager] removeItemAtURL:targetUrl error:nil];

    NSArray<PHAssetResource *> *resources = [PHAssetResource assetResourcesForAsset:asset];
    PHAssetResource *primary = nil;
    for (PHAssetResource *r in resources) {
        if (r.type == PHAssetResourceTypePhoto || r.type == PHAssetResourceTypeFullSizePhoto) {
            primary = r;
            break;
        }
    }
    if (!primary && resources.count > 0) primary = resources.firstObject;

    if (primary) {
        PHAssetResourceRequestOptions *opt = [[PHAssetResourceRequestOptions alloc] init];
        opt.networkAccessAllowed = YES;
        dispatch_semaphore_t sem = dispatch_semaphore_create(0);
        __block NSError *writeErr = nil;
        [[PHAssetResourceManager defaultManager] writeDataForAssetResource:primary toFile:targetUrl options:opt completionHandler:^(NSError * _Nullable error) {
            writeErr = error;
            dispatch_semaphore_signal(sem);
        }];
        dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 15 * NSEC_PER_SEC));
        if (!writeErr && [[NSFileManager defaultManager] fileExistsAtPath:nsTarget]) {
            return 1;
        }
    }

    // Fallback via requestImageDataAndOrientation
    PHImageRequestOptions *opt = [[PHImageRequestOptions alloc] init];
    opt.synchronous = YES;
    opt.deliveryMode = PHImageRequestOptionsDeliveryModeHighQualityFormat;
    opt.networkAccessAllowed = YES;

    __block BOOL success = NO;
    if (@available(macOS 10.15, *)) {
        [[PHImageManager defaultManager] requestImageDataAndOrientationForAsset:asset options:opt resultHandler:^(NSData * _Nullable imageData, NSString * _Nullable dataUTI, CGImagePropertyOrientation orientation, NSDictionary * _Nullable info) {
            if (imageData) {
                success = [imageData writeToURL:targetUrl atomically:YES];
            }
        }];
    }
    return success ? 1 : 0;
}

void noki_photos_free_string(char* s) {
    if (s) free(s);
}
