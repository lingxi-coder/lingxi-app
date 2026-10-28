package com.lingxi.code.project;

import static org.junit.Assert.*;
import android.app.Instrumentation;
import android.content.Context;
import android.database.Cursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
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
    @Test public void bothInstalledVariantsGrantTheirOwnTargetAccess() throws Exception {
        Instrumentation instrumentation = InstrumentationRegistry.getInstrumentation();
        Context test = instrumentation.getContext();
        Context target = instrumentation.getTargetContext();
        String authority = WorkspaceTestDocumentsProvider.authority(test);
        assertEquals(test.getPackageName() + ".filesfixtures", authority);
        assertEquals(target.getPackageName(), WorkspaceTestDocumentsProvider.targetPackage(test));
        String command = "am start -W -n " + test.getPackageName() + "/com.lingxi.code.project.WorkspaceTestDocumentsActivity";
        try (ParcelFileDescriptor fd = instrumentation.getUiAutomation().executeShellCommand(command);
             FileInputStream input = new FileInputStream(fd.getFileDescriptor())) {
            ByteArrayOutputStream buffer = new ByteArrayOutputStream();
            byte[] chunk = new byte[1024];
            for (int count; (count = input.read(chunk)) != -1;) buffer.write(chunk, 0, count);
            String result = new String(buffer.toByteArray(), StandardCharsets.UTF_8);
            assertTrue(result, result.contains("Status: ok"));
        }
        Uri tree = DocumentsContract.buildTreeDocumentUri(authority, "root");
        Uri root = DocumentsContract.buildDocumentUriUsingTree(tree, "root");
        try (Cursor cursor = target.getContentResolver().query(root, null, null, null, null)) {
            assertNotNull(cursor);
            assertTrue(cursor.moveToFirst());
        }
        Uri dir = DocumentsContract.createDocument(target.getContentResolver(), root,
                DocumentsContract.Document.MIME_TYPE_DIR, "probe-" + System.nanoTime());
        assertNotNull(dir);
        assertTrue(DocumentsContract.deleteDocument(target.getContentResolver(), dir));
    }
}
