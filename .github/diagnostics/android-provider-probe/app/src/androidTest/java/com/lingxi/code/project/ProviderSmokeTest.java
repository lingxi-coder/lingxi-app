package com.lingxi.code.project;

import static org.junit.Assert.*;

import android.app.Instrumentation;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.pm.PackageManager;
import android.database.Cursor;
import android.net.Uri;
import android.os.Build;
import android.os.ParcelFileDescriptor;
import android.os.Process;
import android.provider.DocumentsContract;
import android.util.Log;
import androidx.test.ext.junit.runners.AndroidJUnit4;
import androidx.test.platform.app.InstrumentationRegistry;
import org.junit.Test;
import org.junit.runner.RunWith;
import java.util.UUID;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

/** Isolated Android device diagnostic for the candidate test provider. */
@RunWith(AndroidJUnit4.class)
public class ProviderSmokeTest {
    private static void invoke(Instrumentation instrumentation, Context test, String fixture,
                               boolean revoke, boolean expectedSuccess) throws Exception {
        Context target = instrumentation.getTargetContext();
        String request = UUID.randomUUID().toString();
        CountDownLatch latch = new CountDownLatch(1);
        AtomicReference<Intent> receipt = new AtomicReference<>();
        BroadcastReceiver receiver = new BroadcastReceiver() {
            @Override public void onReceive(Context context, Intent intent) {
                if (request.equals(intent.getStringExtra(WorkspaceTestDocumentsActivity.EXTRA_REQUEST_ID)) &&
                    receipt.compareAndSet(null, intent)) latch.countDown();
            }
        };
        IntentFilter filter = new IntentFilter(WorkspaceTestDocumentsActivity.ACTION_RESULT);
        if (Build.VERSION.SDK_INT >= 33) target.registerReceiver(receiver, filter, Context.RECEIVER_EXPORTED);
        else target.registerReceiver(receiver, filter);
        String action = revoke ? "-a " + WorkspaceTestDocumentsActivity.ACTION_REVOKE + " " : "";
        String command = "am start " + action + "--es " + WorkspaceTestDocumentsActivity.EXTRA_FIXTURE_ID + " " + fixture +
            " --es " + WorkspaceTestDocumentsActivity.EXTRA_REQUEST_ID + " " + request + " -n " +
            test.getPackageName() + "/com.lingxi.code.project.WorkspaceTestDocumentsActivity";
        try {
            Log.i("LingxiTest", "v4 before UiAutomation " + request);
            android.app.UiAutomation automation = instrumentation.getUiAutomation();
            Log.i("LingxiTest", "v4 before shell " + request);
            try (ParcelFileDescriptor shellOutput = automation.executeShellCommand(command)) {
                assertNotNull(shellOutput);
                Log.i("LingxiTest", "v4 shell returned, awaiting receipt " + request);
                assertTrue("No fixture receipt for " + request, latch.await(20, TimeUnit.SECONDS));
            }
            Intent result = receipt.get();
            assertNotNull(result);
            Log.i("LingxiTest", "v4 received receipt " + request);
            assertEquals(result.getStringExtra(WorkspaceTestDocumentsActivity.EXTRA_ERROR), expectedSuccess,
                result.getBooleanExtra(WorkspaceTestDocumentsActivity.EXTRA_SUCCESS, false));
        } finally { target.unregisterReceiver(receiver); }
    }

    private static void permission(Context context, Uri tree, int expected) {
        for (int mode : new int[] { Intent.FLAG_GRANT_READ_URI_PERMISSION, Intent.FLAG_GRANT_WRITE_URI_PERMISSION }) {
            assertEquals(tree.toString(), expected, context.checkUriPermission(tree, Process.myPid(), Process.myUid(), mode));
        }
    }

    @Test public void isolatedTreesAndClosedFixtureRejectLateGrant() throws Exception {
        Instrumentation instrumentation = InstrumentationRegistry.getInstrumentation();
        Context test = instrumentation.getContext();
        Context target = instrumentation.getTargetContext();
        String authority = WorkspaceTestDocumentsProvider.authority(test);
        assertEquals(target.getPackageName(), WorkspaceTestDocumentsProvider.targetPackage(test));
        // Never revoke a broad grant that may belong to a concurrent test.
        permission(target, DocumentsContract.buildTreeDocumentUri(authority, "root"), PackageManager.PERMISSION_DENIED);
        String firstId = UUID.randomUUID().toString();
        String secondId = UUID.randomUUID().toString();
        Uri first = DocumentsContract.buildTreeDocumentUri(authority, firstId);
        Uri second = DocumentsContract.buildTreeDocumentUri(authority, secondId);
        try {
            invoke(instrumentation, test, firstId, false, true);
            invoke(instrumentation, test, secondId, false, true);
            permission(target, first, PackageManager.PERMISSION_GRANTED);
            permission(target, second, PackageManager.PERMISSION_GRANTED);
            Uri firstDocument = DocumentsContract.buildDocumentUriUsingTree(first, firstId);
            Log.i("LingxiTest", "v4 before create child");
            Uri child = DocumentsContract.createDocument(target.getContentResolver(), firstDocument,
                DocumentsContract.Document.MIME_TYPE_DIR, "child");
            assertNotNull(child);
            Uri childTree = DocumentsContract.buildTreeDocumentUri(authority, DocumentsContract.getDocumentId(child));
            permission(target, childTree, PackageManager.PERMISSION_GRANTED);
            Log.i("LingxiTest", "v4 before delete fixture");
            assertTrue(DocumentsContract.deleteDocument(target.getContentResolver(), firstDocument));
            Log.i("LingxiTest", "v4 after delete fixture");
            permission(target, first, PackageManager.PERMISSION_DENIED);
            permission(target, childTree, PackageManager.PERMISSION_DENIED);
            permission(target, second, PackageManager.PERMISSION_GRANTED);
            invoke(instrumentation, test, firstId, true, true);
            invoke(instrumentation, test, firstId, false, false);
            permission(target, first, PackageManager.PERMISSION_DENIED);
            permission(target, second, PackageManager.PERMISSION_GRANTED);
            Uri secondDocument = DocumentsContract.buildDocumentUriUsingTree(second, secondId);
            try (Cursor cursor = target.getContentResolver().query(secondDocument, null, null, null, null)) {
                assertNotNull(cursor);
                assertTrue(cursor.moveToFirst());
            }
        } finally {
            try { invoke(instrumentation, test, firstId, true, true); }
            finally { invoke(instrumentation, test, secondId, true, true); }
        }
    }
}
