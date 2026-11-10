package dev.kairo.app;

import android.app.Activity;
import android.app.Application;
import android.app.PendingIntent;
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

    /**
     * Gives an APK to the package installer: a session the user confirms (Android asks
     * once to allow installs from Kairo). Returns null when it was handed over, else a
     * message for the user.
     */
    public static String installApk(String path) {
        try {
            android.content.pm.PackageInstaller pi = app.getPackageManager().getPackageInstaller();
            if (!app.getPackageManager().canRequestPackageInstalls()) {
                // Take the user to the setting; they come back and press again.
                Intent s = new Intent(android.provider.Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
                        android.net.Uri.parse("package:" + app.getPackageName()))
                        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
                app.startActivity(s);
                return "Allow Kairo to install updates in the screen that opened, then press the button again.";
            }
            android.content.pm.PackageInstaller.SessionParams p = new android.content.pm.PackageInstaller.SessionParams(
                    android.content.pm.PackageInstaller.SessionParams.MODE_FULL_INSTALL);
            p.setRequireUserAction(android.content.pm.PackageInstaller.SessionParams.USER_ACTION_NOT_REQUIRED);
            int id = pi.createSession(p);
            try (android.content.pm.PackageInstaller.Session session = pi.openSession(id);
                 java.io.InputStream in = new java.io.FileInputStream(path);
                 java.io.OutputStream out = session.openWrite("kairo.apk", 0, -1)) {
                byte[] buf = new byte[1 << 16];
                int n;
                while ((n = in.read(buf)) > 0) {
                    out.write(buf, 0, n);
                }
                session.fsync(out);
                Intent result = new Intent(app, InstallResultReceiver.class).setPackage(app.getPackageName());
                PendingIntent pending = PendingIntent.getBroadcast(app, id, result,
                        PendingIntent.FLAG_MUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
                session.commit(pending.getIntentSender());
            }
            return null;
        } catch (Exception e) {
            return "Could not start the update: " + e.getMessage();
        }
    }

    /** Stops the VPN and its notification. */
    public static void teardown() {
        KairoVpnService.stop(app);
    }
}
