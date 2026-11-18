package dev.kairo.app;

import android.app.Activity;
import android.app.Application;
import android.os.Bundle;

/**
 * Loads Kairo's native library and starts its engine, whatever started the process:
 * the app, the VPN service (a restart, Always-on, the tile) or a broadcast. The
 * engine does not need a screen; the screens only show it.
 */
public class KairoApp extends Application {
    private boolean opened;

    @Override
    public void onCreate() {
        super.onCreate();
        System.loadLibrary("nsh");
        Bridge.init(this);
        registerActivityLifecycleCallbacks(new ActivityLifecycleCallbacks() {
            @Override
            public void onActivityStarted(Activity a) {
                Bridge.activityStarted();
                if (Bridge.connectWhenOpen) {
                    // The tile opened Kairo to ask for the VPN permission: connect,
                    // and Android's dialog shows over Kairo.
                    Bridge.connectWhenOpen = false;
                    opened = true;
                    Bridge.nativeConnect(true);
                } else if (!opened) {
                    // Opening Kairo connects it when it was on, as it always has.
                    opened = true;
                    Bridge.nativeRestore();
                }
            }

            @Override
            public void onActivityStopped(Activity a) {
                Bridge.activityStopped();
            }

            @Override public void onActivityCreated(Activity a, Bundle b) { }
            @Override public void onActivityResumed(Activity a) { }
            @Override public void onActivityPaused(Activity a) { }
            @Override public void onActivitySaveInstanceState(Activity a, Bundle b) { }
            @Override public void onActivityDestroyed(Activity a) { }
        });
    }
}
