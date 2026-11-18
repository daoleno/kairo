package dev.kairo.app;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;

/**
 * The phone started, or Android updated Kairo (which stops it and does not start it
 * again): Kairo is on again if it was on, without opening the app. The service asks
 * the engine, and stops by itself when Kairo was off.
 */
public class UpdatedReceiver extends BroadcastReceiver {
    @Override
    public void onReceive(Context context, Intent intent) {
        String action = intent.getAction();
        if (!Intent.ACTION_BOOT_COMPLETED.equals(action) && !Intent.ACTION_MY_PACKAGE_REPLACED.equals(action)) {
            return;
        }
        if (!Bridge.wantsOn()) {
            return;
        }
        try {
            KairoVpnService.begin(context, KairoVpnService.RESTORE);
        } catch (Exception e) {
            android.util.Log.w("Kairo", "restore after " + action + " failed", e);
        }
    }
}
