package com.lingxi.teamsspike;

import com.google.firebase.FirebaseApp;
import com.google.firebase.firestore.FirebaseFirestore;
import com.google.firebase.firestore.Query;

public final class FirestoreProbe {
    public static Query check(FirebaseApp app) {
        return FirebaseFirestore.getInstance(app).collection("teams").document("synthetic")
            .collection("conversations").document("synthetic").collection("changes")
            .whereGreaterThan("seq", 1).orderBy("seq");
    }
}
