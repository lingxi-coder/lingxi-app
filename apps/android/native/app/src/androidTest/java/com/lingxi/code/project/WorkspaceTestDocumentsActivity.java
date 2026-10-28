package com.lingxi.code.project;

/** Starts the test APK and its isolated provider, then returns immediately. */
public class WorkspaceTestDocumentsActivity extends android.app.Activity {
    static final String ACTION_REVOKE = "com.lingxi.code.project.REVOKE_TEST_DOCUMENT_GRANTS";
    @Override public void onCreate(android.os.Bundle state) {
        super.onCreate(state);
        // Force-stopping the target revokes transient grants while the test provider may stay alive.
        String target = WorkspaceTestDocumentsProvider.targetPackage(this);
        android.net.Uri tree = android.provider.DocumentsContract.buildTreeDocumentUri(WorkspaceTestDocumentsProvider.authority(this), "root");
        int modes = android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION | android.content.Intent.FLAG_GRANT_WRITE_URI_PERMISSION;
        if (ACTION_REVOKE.equals(getIntent().getAction())) revokeUriPermission(target, tree, modes);
        else grantUriPermission(target, tree, modes | android.content.Intent.FLAG_GRANT_PREFIX_URI_PERMISSION);
        finish();
    }
}
