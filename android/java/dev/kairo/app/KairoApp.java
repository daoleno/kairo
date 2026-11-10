package dev.kairo.app;

import android.app.Application;

/** Loads Kairo's native library and tells it where the app's files are. */
public class KairoApp extends Application {
    @Override
    public void onCreate() {
        super.onCreate();
        System.loadLibrary("nsh");
        Bridge.init(this);
    }
}
