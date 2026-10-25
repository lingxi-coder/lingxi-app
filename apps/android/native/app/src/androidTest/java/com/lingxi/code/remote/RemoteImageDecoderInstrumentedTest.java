package com.lingxi.code.remote;

import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.Color;
import android.graphics.ImageDecoder;
import java.io.ByteArrayOutputStream;
import java.nio.ByteBuffer;
import org.junit.Test;

/** Runs through the real Android decoder, including EXIF reflection and rotation. */
public final class RemoteImageDecoderInstrumentedTest {
    private static final int[] COLORS = {Color.RED, Color.GREEN, Color.BLUE, Color.YELLOW};
    private static final int[][] CORNERS = {
        {0, 1, 2, 3}, {1, 0, 3, 2}, {3, 2, 1, 0}, {2, 3, 0, 1},
        {0, 2, 1, 3}, {2, 0, 3, 1}, {3, 1, 2, 0}, {1, 3, 0, 2},
    };

    @Test public void boundedDecodePreservesAllEightExifOrientations() throws Exception {
        for (int orientation = 1; orientation <= 8; orientation++) {
            byte[] jpeg = fixture(orientation);
            // The old BitmapFactory path always emitted these unrotated pixels.
            Bitmap raw = BitmapFactory.decodeByteArray(jpeg, 0, jpeg.length);
            check(raw.getWidth() == 60 && raw.getHeight() == 30, "raw fixture bounds");
            raw.recycle();
            Bitmap image = RemoteImageDecoder.decode(ImageDecoder.createSource(ByteBuffer.wrap(jpeg)), 32);
            try {
                boolean rotated = orientation >= 5;
                check(image.getWidth() == (rotated ? 16 : 32), "width for EXIF " + orientation);
                check(image.getHeight() == (rotated ? 32 : 16), "height for EXIF " + orientation);
                for (int corner = 0; corner < 4; corner++) {
                    int x = image.getWidth() * (corner % 2 == 0 ? 1 : 3) / 4;
                    int y = image.getHeight() * (corner < 2 ? 1 : 3) / 4;
                    check(nearestColor(image.getPixel(x, y)) == CORNERS[orientation - 1][corner],
                        "corner " + corner + " for EXIF " + orientation);
                }
            } finally { image.recycle(); }
        }
    }

    @Test public void smallImagesAreNotEnlarged() throws Exception {
        Bitmap image = RemoteImageDecoder.decode(ImageDecoder.createSource(ByteBuffer.wrap(fixture(6))), 1568);
        try { check(image.getWidth() == 30 && image.getHeight() == 60, "small image enlarged"); }
        finally { image.recycle(); }
    }

    private static byte[] fixture(int orientation) throws Exception {
        Bitmap image = Bitmap.createBitmap(60, 30, Bitmap.Config.ARGB_8888);
        ByteArrayOutputStream encoded = new ByteArrayOutputStream();
        try {
            for (int y = 0; y < 30; y++) for (int x = 0; x < 60; x++) {
                image.setPixel(x, y, COLORS[(y < 15 ? 0 : 2) + (x < 30 ? 0 : 1)]);
            }
            check(image.compress(Bitmap.CompressFormat.JPEG, 100, encoded), "encode fixture");
        } finally { image.recycle(); }
        byte[] jpeg = encoded.toByteArray();
        byte[] exif = {(byte) 0xff, (byte) 0xe1, 0, 34, 'E', 'x', 'i', 'f', 0, 0,
            'I', 'I', 42, 0, 8, 0, 0, 0, 1, 0, 18, 1, 3, 0, 1, 0, 0, 0,
            (byte) orientation, 0, 0, 0, 0, 0, 0, 0};
        ByteArrayOutputStream result = new ByteArrayOutputStream();
        result.write(jpeg, 0, 2); result.write(exif); result.write(jpeg, 2, jpeg.length - 2);
        return result.toByteArray();
    }

    private static int nearestColor(int value) {
        int best = 0, distance = Integer.MAX_VALUE;
        for (int i = 0; i < COLORS.length; i++) {
            int r = Color.red(value) - Color.red(COLORS[i]);
            int g = Color.green(value) - Color.green(COLORS[i]);
            int b = Color.blue(value) - Color.blue(COLORS[i]);
            int next = r * r + g * g + b * b;
            if (next < distance) { best = i; distance = next; }
        }
        return best;
    }

    private static void check(boolean value, String message) {
        if (!value) throw new AssertionError(message);
    }

    /** Headless device runner: no application install, account, UI or user files. */
    public static void main(String[] args) throws Exception {
        RemoteImageDecoderInstrumentedTest test = new RemoteImageDecoderInstrumentedTest();
        test.boundedDecodePreservesAllEightExifOrientations();
        test.smallImagesAreNotEnlarged();
        System.out.println("PASS: EXIF orientations 1-8, bounded dimensions, corner colors, small-image preservation");
    }
}
