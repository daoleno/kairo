package dev.kairo.app;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;

/**
 * Android closes an app it updates and does not open it again; this says so with a
 * notification that opens Kairo (and with it the VPN, which the update had stopped).
 */
public class UpdatedReceiver extends BroadcastReceiver {
    private static final String CHANNEL = "kairo_updates";

    @Override
    public void onReceive(Context context, Intent intent) {
        NotificationManager nm = context.getSystemService(NotificationManager.class);
        nm.createNotificationChannel(new NotificationChannel(CHANNEL, "Kairo updates", NotificationManager.IMPORTANCE_DEFAULT));
        Intent open = context.getPackageManager().getLaunchIntentForPackage(context.getPackageName());
        if (open == null) {
            return;
        }
        PendingIntent tap = PendingIntent.getActivity(context, 1, open, PendingIntent.FLAG_IMMUTABLE);
        Notification n = new Notification.Builder(context, CHANNEL)
                .setSmallIcon(android.R.drawable.stat_sys_download_done)
                .setContentTitle("Kairo was updated")
                .setContentText("Tap to open it and connect again")
                .setContentIntent(tap)
                .setAutoCancel(true)
                .build();
        nm.notify(2, n);
    }
}
