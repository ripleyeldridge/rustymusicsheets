package org.ripleyeldridge.rustymusicsheets.fixture;

import android.app.Activity;
import android.content.ClipData;
import android.content.ComponentName;
import android.content.Intent;
import android.net.Uri;
import android.os.Bundle;

/** Test-only entry point; cannot target the production app or arbitrary files. */
public final class FixtureActivity extends Activity {
    public static final String TARGET = "org.ripleyeldridge.rustymusicsheets.smoketest";

    @Override public void onCreate(Bundle saved) {
        super.onCreate(saved);
        String fixture = getIntent().getStringExtra("fixture");
        if (!FixtureProvider.isFixture(fixture)) {
            finish();
            return;
        }
        Uri uri = new Uri.Builder().scheme("content")
            .authority(FixtureProvider.AUTHORITY).appendPath(fixture).build();
        Intent view = new Intent(Intent.ACTION_VIEW)
            .setComponent(new ComponentName(TARGET, "android.app.NativeActivity"))
            .setDataAndType(uri, FixtureProvider.mime(fixture))
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        view.setClipData(ClipData.newRawUri("Original test score", uri));
        if (!"denied".equals(fixture)) {
            view.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
        }
        try {
            startActivity(view);
        } finally {
            finish();
        }
    }
}
