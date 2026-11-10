package dev.kairo.app;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;

/** The notification's Disconnect button: the same as turning Kairo off in the app. */
public class DisconnectReceiver extends BroadcastReceiver {
    @Override
    public void onReceive(Context context, Intent intent) {
        Bridge.nativeDisconnect();
    }
}
