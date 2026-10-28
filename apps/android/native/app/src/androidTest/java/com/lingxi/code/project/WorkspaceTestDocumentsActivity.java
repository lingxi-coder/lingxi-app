package com.lingxi.code.project;

/** Starts the test APK and its isolated provider, then returns immediately. */
public class WorkspaceTestDocumentsActivity extends android.app.Activity {
    static final String ACTION_REVOKE = "com.lingxi.code.project.REVOKE_TEST_DOCUMENT_GRANTS";
    static final String ACTION_RESULT = "com.lingxi.code.project.TEST_DOCUMENT_GRANT_RESULT";
    static final String EXTRA_FIXTURE_ID = "fixture_id";
    static final String EXTRA_REQUEST_ID = "request_id";
    static final String EXTRA_SUCCESS = "success";
    static final String EXTRA_ERROR = "error";
    @Override public void onCreate(android.os.Bundle state) {
        super.onCreate(state);
        String request = getIntent().getStringExtra(EXTRA_REQUEST_ID);
        String id = getIntent().getStringExtra(EXTRA_FIXTURE_ID);
        boolean revoke = ACTION_REVOKE.equals(getIntent().getAction());
        // A slow action on one fixture must not block the Activity main thread
        // from accepting work for another fixture.
        new Thread(() -> runFixtureAction(id, request, revoke), "test-document-fixture").start();
    }
    private void runFixtureAction(String id, String request, boolean revoke) {
        String target = null;
        boolean success = false;
        String failure = null;
        android.util.Log.i("LingxiTest", "Fixture action started: " + request);
        try {
            target = WorkspaceTestDocumentsProvider.targetPackage(this);
            if (revoke) {
                WorkspaceTestDocumentsProvider.cleanupFixture(this, id);
            } else {
                WorkspaceTestDocumentsProvider.grantFixture(this, id);
            }
            success = true;
            android.util.Log.i("LingxiTest", "Fixture action completed: " + request);
        } catch (Exception error) {
            failure = error.toString();
            android.util.Log.e("LingxiTest", "Test fixture action failed", error);
        } finally {
            try {
                if (target != null && request != null) {
                    android.content.Intent result = new android.content.Intent(ACTION_RESULT)
                        .setPackage(target)
                        .putExtra(EXTRA_REQUEST_ID, request)
                        .putExtra(EXTRA_SUCCESS, success)
                        .putExtra(EXTRA_ERROR, failure);
                    sendBroadcast(result);
                    android.util.Log.i("LingxiTest", "Fixture receipt sent: " + request);
                }
            } finally { runOnUiThread(this::finish); }
        }
    }
}
