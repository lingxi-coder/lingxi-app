package com.lingxi.code.project;

import android.content.Intent;
import android.content.ComponentName;
import android.content.Context;
import android.content.pm.InstrumentationInfo;
import android.content.pm.PackageManager;
import android.database.Cursor;
import android.database.MatrixCursor;
import android.os.CancellationSignal;
import android.os.ParcelFileDescriptor;
import android.provider.DocumentsContract;
import android.provider.DocumentsProvider;
import java.io.File;
import java.io.FileNotFoundException;
import java.io.IOException;

/** Isolated SAF provider installed exclusively in the test APK. */
public class WorkspaceTestDocumentsProvider extends DocumentsProvider {
    static String authority(Context context) {
        try {
            return context.getPackageManager().getProviderInfo(
                new ComponentName(context, WorkspaceTestDocumentsProvider.class), 0).authority;
        } catch (PackageManager.NameNotFoundException error) {
            throw new IllegalStateException("Test documents provider is not installed", error);
        }
    }
    static String targetPackage(Context context) {
        try {
            InstrumentationInfo[] runners = context.getPackageManager().getPackageInfo(
                context.getPackageName(), PackageManager.GET_INSTRUMENTATION).instrumentation;
            if (runners != null) for (InstrumentationInfo runner : runners) {
                if ("androidx.test.runner.AndroidJUnitRunner".equals(runner.name)) return runner.targetPackage;
            }
        } catch (PackageManager.NameNotFoundException error) {
            throw new IllegalStateException("Test package is not installed", error);
        }
        throw new IllegalStateException("Test instrumentation target is missing");
    }
    private File root() { File root = new File(getContext().getFilesDir(), "workspace-fixtures"); root.mkdirs(); return root; }
    @Override public boolean onCreate() { return true; }
    private File file(String id) {
        try {
            File base = root().getCanonicalFile(); File file = id.equals("root") ? base : new File(base, id).getCanonicalFile();
            if (!file.equals(base) && !file.toPath().startsWith(base.toPath())) throw new IllegalArgumentException("Outside test root");
            return file;
        } catch (IOException error) { throw new IllegalArgumentException(error); }
    }
    private String id(File file) { return file.equals(file("root")) ? "root" : file("root").toPath().relativize(file.toPath()).toString(); }
    private MatrixCursor cursor(String[] projection) { return new MatrixCursor(projection == null ? new String[] { DocumentsContract.Document.COLUMN_DOCUMENT_ID, DocumentsContract.Document.COLUMN_DISPLAY_NAME,
        DocumentsContract.Document.COLUMN_MIME_TYPE, DocumentsContract.Document.COLUMN_FLAGS, DocumentsContract.Document.COLUMN_SIZE, DocumentsContract.Document.COLUMN_LAST_MODIFIED } : projection); }
    private void add(MatrixCursor cursor, File file) {
        String mime = android.webkit.MimeTypeMap.getSingleton().getMimeTypeFromExtension(file.getName().substring(file.getName().lastIndexOf('.') + 1));
        cursor.newRow().add(DocumentsContract.Document.COLUMN_DOCUMENT_ID, id(file)).add(DocumentsContract.Document.COLUMN_DISPLAY_NAME, file.getName())
            .add(DocumentsContract.Document.COLUMN_MIME_TYPE, file.isDirectory() ? DocumentsContract.Document.MIME_TYPE_DIR : mime == null ? "application/octet-stream" : mime)
            .add(DocumentsContract.Document.COLUMN_FLAGS, DocumentsContract.Document.FLAG_SUPPORTS_DELETE | DocumentsContract.Document.FLAG_SUPPORTS_RENAME | (file.isDirectory() ? DocumentsContract.Document.FLAG_DIR_SUPPORTS_CREATE : DocumentsContract.Document.FLAG_SUPPORTS_WRITE))
            .add(DocumentsContract.Document.COLUMN_SIZE, file.length()).add(DocumentsContract.Document.COLUMN_LAST_MODIFIED, file.lastModified());
    }
    @Override public Cursor queryRoots(String[] projection) {
        MatrixCursor cursor = new MatrixCursor(projection == null ? new String[]{DocumentsContract.Root.COLUMN_ROOT_ID, DocumentsContract.Root.COLUMN_DOCUMENT_ID, DocumentsContract.Root.COLUMN_TITLE, DocumentsContract.Root.COLUMN_FLAGS} : projection);
        cursor.newRow().add(DocumentsContract.Root.COLUMN_ROOT_ID, "test").add(DocumentsContract.Root.COLUMN_DOCUMENT_ID, "root").add(DocumentsContract.Root.COLUMN_TITLE, "LingXi Test Fixtures")
            .add(DocumentsContract.Root.COLUMN_FLAGS, DocumentsContract.Root.FLAG_SUPPORTS_CREATE | DocumentsContract.Root.FLAG_LOCAL_ONLY); return cursor;
    }
    @Override public Cursor queryDocument(String documentId, String[] projection) { MatrixCursor cursor = cursor(projection); add(cursor, file(documentId)); return cursor; }
    @Override public Cursor queryChildDocuments(String parent, String[] projection, String sort) { MatrixCursor cursor = cursor(projection); File[] files = file(parent).listFiles(); if (files != null) for (File file : files) add(cursor, file); return cursor; }
    @Override public ParcelFileDescriptor openDocument(String id, String mode, CancellationSignal signal) throws FileNotFoundException { return ParcelFileDescriptor.open(file(id), ParcelFileDescriptor.parseMode(mode)); }
    private void validName(String name) { if (name.isEmpty() || name.equals(".") || name.equals("..") || name.contains("/")) throw new IllegalArgumentException("Invalid name"); }
    @Override public String createDocument(String parent, String mime, String name) throws FileNotFoundException {
        validName(name); File target = new File(file(parent), name);
        try { if (target.exists() || !(mime.equals(DocumentsContract.Document.MIME_TYPE_DIR) ? target.mkdir() : target.createNewFile())) throw new FileNotFoundException(name); }
        catch (IOException error) { throw new FileNotFoundException(error.toString()); }
        if (target.isDirectory()) getContext().grantUriPermission(targetPackage(getContext()),
            DocumentsContract.buildTreeDocumentUri(authority(getContext()), id(target)),
            Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION | Intent.FLAG_GRANT_PREFIX_URI_PERMISSION);
        return id(target);
    }
    @Override public String renameDocument(String old, String name) throws FileNotFoundException { validName(name); File source = file(old); File target = new File(source.getParentFile(), name); if (target.exists() || !source.renameTo(target)) throw new FileNotFoundException(name); return id(target); }
    private void remove(File file) throws FileNotFoundException {
        String documentId = id(file);
        boolean directory = file.isDirectory();
        File[] children = file.listFiles();
        if (children != null) for (File child : children) remove(child);
        if (!file.delete()) throw new FileNotFoundException(file.toString());
        if (directory) getContext().revokeUriPermission(targetPackage(getContext()),
            DocumentsContract.buildTreeDocumentUri(authority(getContext()), documentId),
            Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
    }
    @Override public void deleteDocument(String id) throws FileNotFoundException { if (id.equals("root")) throw new IllegalArgumentException(); remove(file(id)); }
    @Override public boolean isChildDocument(String parent, String child) { return file(child).toPath().startsWith(file(parent).toPath()); }
}
