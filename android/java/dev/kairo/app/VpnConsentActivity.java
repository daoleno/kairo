package dev.kairo.app;

import android.app.Activity;
import android.content.Intent;
import android.net.VpnService;
import android.os.Bundle;

/** A see-through screen that shows Android's "Allow VPN connection" dialog and reports the answer. */
public class VpnConsentActivity extends Activity {
    private static final int REQUEST = 1;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        Intent prepare = VpnService.prepare(this);
        if (prepare == null) {
            finish(true);
            return;
        }
        if (state == null) {
            startActivityForResult(prepare, REQUEST);
        }
    }

    @Override
    protected void onActivityResult(int request, int result, Intent data) {
        finish(result == RESULT_OK);
    }

    private void finish(boolean ok) {
        if (Bridge.consent != null) {
            Bridge.consent.complete(ok);
        }
        finish();
    }
}
