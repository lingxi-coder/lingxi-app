package com.lingxi.teamsspike;

import android.app.Activity;
import android.os.Bundle;
import android.widget.TextView;
import com.google.firebase.FirebaseApp;
import com.google.firebase.FirebaseOptions;
import com.google.firebase.auth.FirebaseAuth;

public final class MainActivity extends Activity {
    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        FirebaseApp app = FirebaseApp.initializeApp(this, new FirebaseOptions.Builder()
            .setApplicationId("1:123:android:teamsspike").setApiKey("emulator-only")
            .setProjectId("demo-lingxi-teams-t0").build(), "teams-size-probe");
        FirebaseAuth.getInstance(app);
        TextView label = new TextView(this);
        try {
            Class.forName("com.lingxi.teamsspike.FirestoreProbe").getMethod("check", FirebaseApp.class).invoke(null, app);
            label.setText("Firestore SDK linked");
        } catch (ClassNotFoundException baseline) {
            label.setText("Baseline Firebase SDK linked");
        } catch (ReflectiveOperationException failure) {
            throw new IllegalStateException(failure);
        }
        setContentView(label);
    }
}
