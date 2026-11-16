package org.ripleyeldridge.rustymusicsheets.fixture;

import android.content.ContentProvider;
import android.content.ContentValues;
import android.database.Cursor;
import android.database.MatrixCursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.provider.OpenableColumns;
import java.io.File;
import java.io.FileNotFoundException;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.util.HashMap;
import java.util.Map;

/** Private read-only fixture provider; all paths are a fixed asset allowlist. */
public final class FixtureProvider extends ContentProvider {
    public static final String AUTHORITY = "org.ripleyeldridge.rustymusicsheets.fixture.documents";
    private static final Map<String, String> FILES = new HashMap<String, String>();
    static {
        FILES.put("png", "score.png");
        FILES.put("jpeg", "score.jpg");
        FILES.put("unicode", "unicode.png");
        FILES.put("text-pdf", "text-score.pdf");
        FILES.put("multipage-pdf", "multipage.pdf");
        FILES.put("scanned-pdf", "scanned.pdf");
        FILES.put("corrupt", "corrupt.png");
        FILES.put("oversized", "oversized.png");
        FILES.put("oversized-metadata", "score.png");
        FILES.put("unsafe-name", "score.png");
        FILES.put("denied", "score.png");
        FILES.put("pipe", "score.png");
    }
    public static boolean isFixture(String name) { return FILES.containsKey(name); }
    public static String mime(String name) {
        String path = FILES.get(name);
        return path.endsWith(".pdf") ? "application/pdf" :
            path.endsWith(".jpg") ? "image/jpeg" : "image/png";
    }
    @Override public boolean onCreate() { return true; }

    private String fixture(Uri uri) {
        String name = uri.getLastPathSegment();
        if (!AUTHORITY.equals(uri.getAuthority()) || uri.getPathSegments().size() != 1 || !isFixture(name)) {
            throw new IllegalArgumentException("Unknown generated fixture");
        }
        return name;
    }

    private File materialize(String name) throws IOException {
        File destination = new File(getContext().getCacheDir(), FILES.get(name));
        if (!destination.isFile()) {
            try (InputStream input = getContext().getAssets().open(FILES.get(name));
                 FileOutputStream output = new FileOutputStream(destination)) {
                byte[] buffer = new byte[65536];
                int count;
                while ((count = input.read(buffer)) != -1) {
                    output.write(buffer, 0, count);
                }
            }
        }
        return destination;
    }

    @Override public Cursor query(Uri uri, String[] projection, String selection, String[] args, String sort) {
        String name = fixture(uri);
        String[] columns = projection == null ? new String[]{OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE} : projection;
        MatrixCursor cursor = new MatrixCursor(columns);
        MatrixCursor.RowBuilder row = cursor.newRow();
        for (String column : columns) {
            if (OpenableColumns.DISPLAY_NAME.equals(column)) {
                row.add("unsafe-name".equals(name) ? "../escape.png" :
                    "png".equals(name) ? "Smoke PNG.png" :
                    "jpeg".equals(name) ? "Smoke JPEG.jpg" :
                    "unicode".equals(name) ? "Étude Unicode.png" : FILES.get(name));
            } else if (OpenableColumns.SIZE.equals(column)) {
                // Normal fixtures exercise unknown-length provider streams.
                row.add("oversized-metadata".equals(name) ? Long.valueOf(513L * 1024L * 1024L) : null);
            } else {
                row.add(null);
            }
        }
        return cursor;
    }

    @Override public String getType(Uri uri) { return mime(fixture(uri)); }
    @Override public ParcelFileDescriptor openFile(Uri uri, String mode) throws FileNotFoundException {
        if (!"r".equals(mode)) { throw new FileNotFoundException("Read-only fixtures"); }
        try {
            String name = fixture(uri);
            if ("pipe".equals(name)) {
                final ParcelFileDescriptor[] pipe = ParcelFileDescriptor.createPipe();
                new Thread(new Runnable() {
                    @Override public void run() {
                        try (InputStream input = getContext().getAssets().open("score.png");
                             ParcelFileDescriptor.AutoCloseOutputStream output = new ParcelFileDescriptor.AutoCloseOutputStream(pipe[1])) {
                            byte[] bytes = new byte[65536];
                            int count;
                            while ((count = input.read(bytes)) != -1) { output.write(bytes, 0, count); }
                        } catch (IOException ignored) {
                            // Closing the write end propagates EOF/error to the test reader.
                        }
                    }
                }, "rms-fixture-pipe").start();
                return pipe[0];
            }
            return ParcelFileDescriptor.open(materialize(name), ParcelFileDescriptor.MODE_READ_ONLY);
        } catch (IOException error) {
            throw new FileNotFoundException("Generated fixture unavailable");
        }
    }
    @Override public Uri insert(Uri uri, ContentValues values) { throw new UnsupportedOperationException(); }
    @Override public int update(Uri uri, ContentValues values, String selection, String[] args) { throw new UnsupportedOperationException(); }
    @Override public int delete(Uri uri, String selection, String[] args) { throw new UnsupportedOperationException(); }
}
