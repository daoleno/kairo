package dev.kairo.app;

import android.app.Activity;
import android.app.Application;
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

    /** Stops the VPN and its notification. */
    public static void teardown() {
        KairoVpnService.stop(app);
    }
}
