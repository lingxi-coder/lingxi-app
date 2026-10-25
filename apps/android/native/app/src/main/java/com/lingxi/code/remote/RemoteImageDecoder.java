package com.lingxi.code.remote;

import android.graphics.Bitmap;
import android.graphics.ImageDecoder;
import java.io.IOException;

/** Decode to bounded software pixels, applying the source's EXIF rotation and mirroring. */
public final class RemoteImageDecoder {
    private RemoteImageDecoder() {}

    public static Bitmap decode(ImageDecoder.Source source, int maxEdge) throws IOException {
        if (maxEdge < 1) throw new IllegalArgumentException("maxEdge must be positive");
        return ImageDecoder.decodeBitmap(source, (decoder, info, ignored) -> {
            int width = info.getSize().getWidth();
            int height = info.getSize().getHeight();
            double scale = Math.min(1.0, (double) maxEdge / Math.max(width, height));
            decoder.setTargetSize(Math.max(1, (int) Math.round(width * scale)),
                Math.max(1, (int) Math.round(height * scale)));
            decoder.setAllocator(ImageDecoder.ALLOCATOR_SOFTWARE);
        });
    }
}
