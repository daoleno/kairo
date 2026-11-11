package dev.kairo.app;

import android.app.Activity;
import android.app.Application;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.net.VpnService;

import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;

/**
 * The bridge between Kairo's Rust core and what only Java can do.
 *
 * Rust calls {@link #establish()} and {@link #teardown()} (static, from a thread it
 * attached to the VM); the native methods below are implemented in Rust under their
 * JNI names, so nothing is registered by hand.
 */
public final class Bridge {
    private Bridge() {}

    static Application app;

    /** Rust: the app's files directory and native library directory. */
    static native void nativeInit(String filesDir, String nativeLibraryDir);

    /** Rust: the VPN was taken away (the user switched it off in Settings, or another VPN took over). */
    static native void nativeRevoked();

    /** Rust: the notification's Disconnect button was pressed. */
    static native void nativeDisconnect();

    /** Rust: the package installer's last word on an update (a PackageInstaller.STATUS_*, and Android's message). */
    static native void nativeInstallResult(int status, String message);

    static void init(Application a) {
        app = a;
        nativeInit(a.getFilesDir().getAbsolutePath(), a.getApplicationInfo().nativeLibraryDir);
    }

    /** Waited on by {@link #establish()} while the consent screen is up. */
    static volatile CompletableFuture<Boolean> consent;

    /**
     * Brings the VPN up (asking for permission the first time) and returns a file
     * descriptor of its tun device: -1 when the user said no, -2 when Android could
     * not set the VPN up. Blocks; called off the UI thread.
     */
    public static int establish() {
        try {
            askForNotifications();
            if (VpnService.prepare(app) != null) {
                CompletableFuture<Boolean> f = new CompletableFuture<>();
                consent = f;
                Intent i = new Intent(app, VpnConsentActivity.class).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
                app.startActivity(i);
                if (!f.get(180, TimeUnit.SECONDS)) {
                    return -1;
                }
            }
            CompletableFuture<Integer> up = new CompletableFuture<>();
            KairoVpnService.pending = up;
            Intent s = new Intent(app, KairoVpnService.class).setAction(KairoVpnService.START);
            app.startForegroundService(s);
            return up.get(30, TimeUnit.SECONDS);
        } catch (Exception e) {
            return -2;
        }
    }

    /** Waited on by {@link #askForNotifications()} while the permission dialog is up. */
    static volatile CompletableFuture<Boolean> notifications;

    /**
     * The first time Kairo connects, asks (Android 13+) to show notifications: the one
     * that says Kairo is on, with its Disconnect button, is only visible once allowed.
     * Asked once; the answer does not stop the connection.
     */
    private static void askForNotifications() {
        try {
            android.content.SharedPreferences prefs = app.getSharedPreferences("kairo", Context.MODE_PRIVATE);
            if (android.os.Build.VERSION.SDK_INT < 33 || prefs.getBoolean("asked_notifications", false)) {
                return;
            }
            prefs.edit().putBoolean("asked_notifications", true).apply();
            CompletableFuture<Boolean> f = new CompletableFuture<>();
            notifications = f;
            app.startActivity(new Intent(app, NotificationPermissionActivity.class).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
            f.get(120, TimeUnit.SECONDS);
        } catch (Exception ignored) {
            // Connecting matters more than the notification.
        }
    }

    /**
     * Gives an APK to the package installer: a session the user confirms (Android asks
     * once to allow installs from Kairo). Returns null when it was handed over, else a
     * message for the user.
     */
    public static String installApk(String path) {
        int id = -1;
        android.content.pm.PackageInstaller pi = null;
        try {
            pi = app.getPackageManager().getPackageInstaller();
            if (!app.getPackageManager().canRequestPackageInstalls()) {
                // Take the user to the setting; they come back and press again.
                Intent s = new Intent(android.provider.Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
                        android.net.Uri.parse("package:" + app.getPackageName()))
                        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
                app.startActivity(s);
                // The button stays "Update to X": press it again after allowing.
                return null;
            }
            android.content.pm.PackageInstaller.SessionParams p = new android.content.pm.PackageInstaller.SessionParams(
                    android.content.pm.PackageInstaller.SessionParams.MODE_FULL_INSTALL);
            p.setRequireUserAction(android.content.pm.PackageInstaller.SessionParams.USER_ACTION_NOT_REQUIRED);
            id = pi.createSession(p);
            try (android.content.pm.PackageInstaller.Session session = pi.openSession(id)) {
                try (java.io.InputStream in = new java.io.FileInputStream(path);
                     java.io.OutputStream out = session.openWrite("kairo.apk", 0, -1)) {
                    byte[] buf = new byte[1 << 16];
                    int n;
                    while ((n = in.read(buf)) > 0) {
                        out.write(buf, 0, n);
                    }
                    session.fsync(out);
                }
                // Committed only once the file is closed ("Files still open" otherwise).
                Intent result = new Intent(app, InstallResultReceiver.class).setPackage(app.getPackageName());
                PendingIntent pending = PendingIntent.getBroadcast(app, id, result,
                        PendingIntent.FLAG_MUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
                session.commit(pending.getIntentSender());
            }
            return null;
        } catch (Exception e) {
            android.util.Log.e("Kairo", "installApk failed", e);
            if (pi != null && id >= 0) {
                try { pi.abandonSession(id); } catch (Exception ignored) { }
            }
            return "Could not start the update: " + e;
        }
    }

    /**
     * The icon of an installed app as PNG bytes, for the pages of apps (Activity, Apps &amp; Sites):
     * by package name (what the core reports for an app's traffic), else by the app's label.
     * Null when no installed app is called that.
     */
    public static byte[] appIcon(String name) {
        try {
            android.content.pm.PackageManager pm = app.getPackageManager();
            android.graphics.drawable.Drawable icon = null;
            try {
                icon = pm.getApplicationIcon(name);
            } catch (android.content.pm.PackageManager.NameNotFoundException notAPackage) {
                for (android.content.pm.ApplicationInfo info : pm.getInstalledApplications(0)) {
                    if (pm.getApplicationLabel(info).toString().equalsIgnoreCase(name)) {
                        icon = pm.getApplicationIcon(info);
                        break;
                    }
                }
            }
            if (icon == null) {
                return null;
            }
            int side = 128;
            android.graphics.Bitmap bitmap = android.graphics.Bitmap.createBitmap(
                    side, side, android.graphics.Bitmap.Config.ARGB_8888);
            android.graphics.Canvas canvas = new android.graphics.Canvas(bitmap);
            icon.setBounds(0, 0, side, side);
            icon.draw(canvas);
            java.io.ByteArrayOutputStream out = new java.io.ByteArrayOutputStream();
            bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, out);
            return out.toByteArray();
        } catch (Exception e) {
            android.util.Log.w("Kairo", "appIcon failed", e);
            return null;
        }
    }

    /** Stops the VPN and its notification. */
    public static void teardown() {
        KairoVpnService.stop(app);
    }
}
