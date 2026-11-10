package dev.kairo.app;

import android.Manifest;
import android.app.Activity;
import android.content.pm.PackageManager;
import android.os.Build;
import android.os.Bundle;

/** A see-through screen that asks for permission to show notifications (Android 13+) and reports the answer. */
public class NotificationPermissionActivity extends Activity {
    private static final int REQUEST = 2;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        if (Build.VERSION.SDK_INT < 33
                || checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED) {
            done();
            return;
        }
        if (state == null) {
            requestPermissions(new String[] {Manifest.permission.POST_NOTIFICATIONS}, REQUEST);
        }
    }

    @Override
    public void onRequestPermissionsResult(int request, String[] permissions, int[] results) {
        done();
    }

    private void done() {
        if (Bridge.notifications != null) {
            Bridge.notifications.complete(true);
        }
        finish();
    }
}
