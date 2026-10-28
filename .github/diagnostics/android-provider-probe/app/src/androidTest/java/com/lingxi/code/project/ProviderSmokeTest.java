package com.lingxi.code.project;

import static org.junit.Assert.*;
import android.app.Instrumentation;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.database.Cursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.os.Process;
import android.provider.DocumentsContract;
import androidx.test.ext.junit.runners.AndroidJUnit4;
import androidx.test.platform.app.InstrumentationRegistry;
import org.junit.Test;
import org.junit.runner.RunWith;
import java.io.FileInputStream;
import java.io.ByteArrayOutputStream;
import java.nio.charset.StandardCharsets;

@RunWith(AndroidJUnit4.class)
public class ProviderSmokeTest {
    private static void launch(Instrumentation instrumentation, Context test, boolean revoke) throws Exception {
        String action = revoke ? "-a " + WorkspaceTestDocumentsActivity.ACTION_REVOKE + " " : "";
        String command = "am start -W " + action + "-n " + test.getPackageName() + "/com.lingxi.code.project.WorkspaceTestDocumentsActivity";
        try (ParcelFileDescriptor fd = instrumentation.getUiAutomation().executeShellCommand(command);
             FileInputStream input = new FileInputStream(fd.getFileDescriptor())) {
            ByteArrayOutputStream buffer = new ByteArrayOutputStream();
            byte[] chunk = new byte[1024];
            for (int count; (count = input.read(chunk)) != -1;) buffer.write(chunk, 0, count);
            String result = new String(buffer.toByteArray(), StandardCharsets.UTF_8);
            assertTrue(result, result.contains("Status: ok"));
        }
    }
    private static void permission(Context target, Uri tree, int expected) {
        for (int mode : new int[]{Intent.FLAG_GRANT_READ_URI_PERMISSION, Intent.FLAG_GRANT_WRITE_URI_PERMISSION}) {
            assertEquals(tree.toString(), expected, target.checkUriPermission(tree, Process.myPid(), Process.myUid(), mode));
        }
    }
    @Test public void bothInstalledVariantsGrantAndRevokeTheirOwnTargetAccess() throws Exception {
        Instrumentation instrumentation = InstrumentationRegistry.getInstrumentation();
        Context test = instrumentation.getContext();
        Context target = instrumentation.getTargetContext();
        String authority = WorkspaceTestDocumentsProvider.authority(test);
        assertEquals(test.getPackageName() + ".filesfixtures", authority);
        assertEquals(target.getPackageName(), WorkspaceTestDocumentsProvider.targetPackage(test));
        Uri tree = DocumentsContract.buildTreeDocumentUri(authority, "root");
        android.util.Log.i("LingxiTest", "before initial grant");
        launch(instrumentation, test, false);
        android.util.Log.i("LingxiTest", "after grant check");
        permission(target, tree, PackageManager.PERMISSION_GRANTED);
        Uri root = DocumentsContract.buildDocumentUriUsingTree(tree, "root");
        try (Cursor cursor = target.getContentResolver().query(root, null, null, null, null)) {
            assertNotNull(cursor);
            assertTrue(cursor.moveToFirst());
        }
        android.util.Log.i("LingxiTest", "before create");
        Uri dir = DocumentsContract.createDocument(target.getContentResolver(), root,
                DocumentsContract.Document.MIME_TYPE_DIR, "probe-" + System.nanoTime());
        assertNotNull(dir);
        Uri childTree = DocumentsContract.buildTreeDocumentUri(authority, DocumentsContract.getDocumentId(dir));
        permission(target, childTree, PackageManager.PERMISSION_GRANTED);
        android.util.Log.i("LingxiTest", "before delete");
        assertTrue(DocumentsContract.deleteDocument(target.getContentResolver(), dir));
        android.util.Log.i("LingxiTest", "after delete");
        permission(target, childTree, PackageManager.PERMISSION_DENIED);
        android.util.Log.i("LingxiTest", "before revoke");
        launch(instrumentation, test, true);
        permission(target, tree, PackageManager.PERMISSION_DENIED);
        launch(instrumentation, test, false);
        permission(target, tree, PackageManager.PERMISSION_GRANTED);
        launch(instrumentation, test, true);
        permission(target, tree, PackageManager.PERMISSION_DENIED);
    }
}
