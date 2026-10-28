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
import android.system.Os;
import android.os.SystemClock;
import android.util.Log;
import androidx.test.ext.junit.runners.AndroidJUnit4;
import androidx.test.platform.app.InstrumentationRegistry;
import org.junit.Test;
import org.junit.runner.RunWith;
import java.util.UUID;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

/** Isolated Android device diagnostic for the candidate test provider. */
@RunWith(AndroidJUnit4.class)
public class ProviderSmokeTest {
    @FunctionalInterface
    private interface CleanupAction { void run() throws Throwable; }

    private static void runCleanups(Throwable primary, CleanupAction... actions) throws Throwable {
        Throwable first = primary;
        for (CleanupAction action : actions) {
            try { action.run(); }
            catch (Throwable failure) {
                if (first == null) {
                    first = failure;
                    Log.e("LingxiTest", "First fixture cleanup failure", failure);
                } else if (failure != first) {
                    first.addSuppressed(failure);
                    Log.e("LingxiTest", "Additional fixture cleanup failure", failure);
                }
            }
        }
        if (primary == null && first != null) throw first;
    }

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
        Throwable primary = null;
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
        } catch (Exception | Error failure) {
            primary = failure;
            Log.e("LingxiTest", "Fixture action failed for " + request + " revoke=" + revoke, failure);
            throw failure;
        } finally {
            try { target.unregisterReceiver(receiver); }
            catch (RuntimeException | Error failure) {
                Log.e("LingxiTest", "Fixture receiver cleanup failed for " + request, failure);
                if (primary == null) throw failure;
                if (primary != failure) primary.addSuppressed(failure);
            }
        }
    }

    private static void permission(Context context, Uri tree, int expected) {
        for (int mode : new int[] { Intent.FLAG_GRANT_READ_URI_PERMISSION, Intent.FLAG_GRANT_WRITE_URI_PERMISSION }) {
            assertEquals(tree.toString(), expected, context.checkUriPermission(tree, Process.myPid(), Process.myUid(), mode));
        }
    }

    private static void waitForGate(ParcelFileDescriptor gate, long offset, byte expected) throws Exception {
        byte[] one = new byte[1];
        long deadline = SystemClock.elapsedRealtime() + 10000;
        while (SystemClock.elapsedRealtime() < deadline) {
            if (Os.pread(gate.getFileDescriptor(), one, 0, 1, offset) == 1 && one[0] == expected) return;
            SystemClock.sleep(10);
        }
        fail("Gate did not reach " + (char)expected);
    }

    private static void concurrentCleanupWaitsForChildGrant(Instrumentation instrumentation, Context test,
                                                             Context target, String authority) throws Throwable {
        String firstId = UUID.randomUUID().toString();
        String secondId = UUID.randomUUID().toString();
        Uri first = DocumentsContract.buildTreeDocumentUri(authority, firstId);
        Uri second = DocumentsContract.buildTreeDocumentUri(authority, secondId);
        ExecutorService pool = Executors.newFixedThreadPool(2);
        ParcelFileDescriptor gate = null;
        Throwable primary = null;
        try {
            invoke(instrumentation, test, firstId, false, true);
            Uri firstDocument = DocumentsContract.buildDocumentUriUsingTree(first, firstId);
            Uri gateDocument = DocumentsContract.createDocument(target.getContentResolver(), firstDocument,
                "text/plain", ".lingxi-diagnostic-gate");
            assertNotNull(gateDocument);
            gate = target.getContentResolver().openFileDescriptor(gateDocument, "rw");
            assertNotNull(gate);
            Os.pwrite(gate.getFileDescriptor(), new byte[] { 0, 0, 0 }, 0, 3, 0);
            Future<Uri> child = pool.submit(() -> DocumentsContract.createDocument(target.getContentResolver(), firstDocument,
                DocumentsContract.Document.MIME_TYPE_DIR, ".lingxi-diagnostic-held-child"));
            waitForGate(gate, 0, (byte)'E');
            Future<?> cleanup = pool.submit(() -> { invoke(instrumentation, test, firstId, true, true); return null; });
            waitForGate(gate, 2, (byte)'C');
            assertFalse("cleanup completed while a child grant was paused", cleanup.isDone());
            // The Activity main thread must still accept actions for an unrelated fixture.
            invoke(instrumentation, test, secondId, false, true);
            permission(target, second, PackageManager.PERMISSION_GRANTED);
            Os.pwrite(gate.getFileDescriptor(), new byte[] { (byte)'R' }, 0, 1, 1);
            Uri childDocument = child.get(10, TimeUnit.SECONDS);
            assertNotNull(childDocument);
            cleanup.get(10, TimeUnit.SECONDS);
            Uri childTree = DocumentsContract.buildTreeDocumentUri(authority, DocumentsContract.getDocumentId(childDocument));
            permission(target, first, PackageManager.PERMISSION_DENIED);
            permission(target, childTree, PackageManager.PERMISSION_DENIED);
            permission(target, second, PackageManager.PERMISSION_GRANTED);
            Log.i("LingxiTest", "v5 concurrent cleanup gate passed");
        } catch (Throwable failure) {
            primary = failure;
            Log.e("LingxiTest", "First concurrent fixture failure", failure);
            throw failure;
        } finally {
            ParcelFileDescriptor activeGate = gate;
            runCleanups(primary,
                () -> { if (activeGate != null) Os.pwrite(activeGate.getFileDescriptor(), new byte[] { (byte)'R' }, 0, 1, 1); },
                () -> { if (activeGate != null) activeGate.close(); },
                () -> pool.shutdownNow(),
                () -> invoke(instrumentation, test, firstId, true, true),
                () -> invoke(instrumentation, test, secondId, true, true));
        }
    }

    @Test public void isolatedTreesAndClosedFixtureRejectLateGrant() throws Throwable {
        try { runSmoke(); }
        catch (Throwable failure) {
            Log.e("LingxiTest", "Provider smoke failed", failure);
            throw failure;
        }
    }

    private static void runSmoke() throws Throwable {
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
        Throwable primary = null;
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
        } catch (Throwable failure) {
            primary = failure;
            Log.e("LingxiTest", "First ordinary fixture failure", failure);
            throw failure;
        } finally {
            runCleanups(primary,
                () -> invoke(instrumentation, test, firstId, true, true),
                () -> invoke(instrumentation, test, secondId, true, true));
        }
        concurrentCleanupWaitsForChildGrant(instrumentation, test, target, authority);
    }
}
