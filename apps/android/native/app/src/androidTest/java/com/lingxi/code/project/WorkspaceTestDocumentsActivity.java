package com.lingxi.code.project;

/** Starts the test APK and its isolated provider, then returns immediately. */
public class WorkspaceTestDocumentsActivity extends android.app.Activity {
    @Override public void onCreate(android.os.Bundle state) {
        super.onCreate(state);
        // Force-stopping the target revokes transient grants while the test provider may stay alive.
        grantUriPermission("com.lingxi.code.debug", android.provider.DocumentsContract.buildTreeDocumentUri(
            "com.lingxi.code.debug.test.filesfixtures", "root"), android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION
            | android.content.Intent.FLAG_GRANT_WRITE_URI_PERMISSION | android.content.Intent.FLAG_GRANT_PREFIX_URI_PERMISSION);
        finish();
    }
}
