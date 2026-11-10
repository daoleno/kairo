package dev.kairo.app;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.content.pm.ServiceInfo;
import android.net.VpnService;
import android.os.ParcelFileDescriptor;

import java.util.concurrent.CompletableFuture;

/**
 * The VPN: it makes the tun device and keeps it (and a notification) alive. The
 * Rust core, in this same process, is given the descriptor and does the routing.
 */
public class KairoVpnService extends VpnService {
    static final String START = "dev.kairo.app.START";
    static final String STOP = "dev.kairo.app.STOP";
    private static final String CHANNEL = "kairo_vpn";
    private static final int NOTIFICATION = 1;

    /** The answer {@link Bridge#establish()} waits for: the descriptor, or a negative code. */
    static volatile CompletableFuture<Integer> pending;
    private static volatile KairoVpnService running;

    private ParcelFileDescriptor tun;

    static void stop(Context c) {
        KairoVpnService s = running;
        if (s != null) {
            s.close();
        }
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        if (intent != null && STOP.equals(intent.getAction())) {
            close();
            return START_NOT_STICKY;
        }
        startInForeground();
        CompletableFuture<Integer> answer = pending;
        try {
            Builder b = new Builder()
                    .setSession("Kairo")
                    .setMtu(1500)
                    .addAddress("172.19.0.1", 30)
                    .addRoute("0.0.0.0", 0)
                    .addAddress("fdfe:dcba:9876::1", 126)
                    .addRoute("::", 0)
                    .addDnsServer("172.19.0.2")
                    .setMetered(false);
            // The core and Kairo's own requests must not loop back into the VPN.
            b.addDisallowedApplication(getPackageName());
            ParcelFileDescriptor old = tun;
            tun = b.establish();
            if (old != null) {
                try { old.close(); } catch (Exception ignored) { }
            }
            running = this;
            if (answer != null) {
                answer.complete(tun == null ? -2 : tun.getFd());
            }
        } catch (Exception e) {
            if (answer != null) {
                answer.complete(-2);
            }
            stopSelf();
        }
        return START_NOT_STICKY;
    }

    private void startInForeground() {
        NotificationManager nm = getSystemService(NotificationManager.class);
        nm.createNotificationChannel(new NotificationChannel(CHANNEL, "Kairo is on", NotificationManager.IMPORTANCE_LOW));
        Intent open = getPackageManager().getLaunchIntentForPackage(getPackageName());
        PendingIntent tap = PendingIntent.getActivity(this, 0, open, PendingIntent.FLAG_IMMUTABLE);
        PendingIntent off = PendingIntent.getBroadcast(this, 0,
                new Intent(this, DisconnectReceiver.class).setPackage(getPackageName()), PendingIntent.FLAG_IMMUTABLE);
        Notification n = new Notification.Builder(this, CHANNEL)
                .setSmallIcon(android.R.drawable.ic_lock_lock)
                .setContentTitle("Kairo is on")
                .setContentText("Your apps go through Kairo")
                .setContentIntent(tap)
                .addAction(new Notification.Action.Builder(null, "Disconnect", off).build())
                .setOngoing(true)
                .build();
        startForeground(NOTIFICATION, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE);
    }

    private void close() {
        running = null;
        ParcelFileDescriptor t = tun;
        tun = null;
        if (t != null) {
            try { t.close(); } catch (Exception ignored) { }
        }
        stopForeground(STOP_FOREGROUND_REMOVE);
        stopSelf();
    }

    @Override
    public void onRevoke() {
        close();
        Bridge.nativeRevoked();
    }

    @Override
    public void onDestroy() {
        close();
        super.onDestroy();
    }
}
