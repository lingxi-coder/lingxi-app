package com.lingxi.code.apps

import com.lingxi.code.model.SessionMode
import com.lingxi.code.project.ProjectAtomicWriter
import kotlinx.coroutines.runBlocking
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.nio.file.Files
import java.security.MessageDigest

/** The fixture is emitted by the real mobile native handler, not a handwritten response. */
class NativeAuthoringHandlerFixtureTest {
    private fun fixture(): JSONObject {
        val bytes = checkNotNull(javaClass.getResourceAsStream("/fixtures/native-authoring-handler.json")).use { it.readBytes() }
        val digest = MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
        assertEquals("9cea0f8bbd647586c0ccc4bf999d5d2a0470f8db549b0d256c3e4e0f46bd0277", digest)
        return JSONObject(bytes.toString(Charsets.UTF_8))
    }

    private fun response(root: JSONObject, name: String): JSONObject =
        decodeAppResponse(root.getJSONObject(name).getJSONObject("response").toString()) as JSONObject

    private fun normalized(value: Any?): Any? = when (value) {
        is JSONObject -> value.keys().asSequence().toList().sorted().associateWith { normalized(value.get(it)) }
        is JSONArray -> (0 until value.length()).map { normalized(value.get(it)) }
        JSONObject.NULL -> null
        else -> value
    }

    @Test fun nativeCreateProofAndReceiptsRetainTheExactOwner() {
        val root = fixture()
        val created = response(root, "create")
        val proof = response(root, "sourceProof")
        val register = root.getJSONObject("register").getJSONObject("request")
        val owner = register.getJSONObject("authoring")
        val source = created.getString("sourceDirectory")
        val sourceId = created.getString("sourceId")
        assertTrue(isCanonicalUuidValue(sourceId))
        assertEquals(register.getString("requestId"), created.getString("requestId"))
        assertEquals(sourceId, proof.getString("sourceId"))
        assertEquals(source, proof.getString("sourceDirectory"))
        assertEquals(created.getString("sourcePath"), proof.getString("sourcePath"))
        assertEquals(sourceId, register.getString("sourceId"))
        val project = owner.getJSONObject("projectRef").getString("projectId")
        val workspace = owner.getString("workspaceId")
        val session = owner.getString("authoringSessionId")
        for (name in listOf("register", "get")) {
            val grant = response(root, name).appAuthoringGrant(sourceId)
            assertTrue(grant.matchesOwner(sourceId, source, project, workspace, session))
            assertNull(grant.ownerAppId)
            assertFalse(grant.matchesOwner("123e4567-e89b-12d3-a456-426614174000", source, project, workspace, session))
        }
        val prepared = response(root, "prepare")
        assertEquals("resumeSession", prepared.getString("action"))
        assertEquals(sourceId, prepared.getString("sourceId"))
        assertEquals(source, prepared.getString("sourceDirectory"))
        assertEquals(created.getString("sourcePath"), prepared.getString("sourcePath"))
        val stored = response(root, "get").appAuthoringGrant().authoring
        val current = prepared.getJSONObject("authoring").appAuthoring()
        assertEquals(stored.copy(appId = null), current.copy(appId = null))
        assertEquals("sourceMissing", response(root, "sourceMissing").getString("action"))
    }

    @Test fun installedRemixPersistsTheNewAppAndOriginalRequest() = runBlocking {
        val root = fixture()
        val nativeRequest = root.getJSONObject("installedRemix").getJSONObject("request")
        val value = response(root, "installedRemix")
        val record = value.getJSONObject("record").appRecord()
        val oldAppId = nativeRequest.getString("appId")
        val requestId = nativeRequest.getString("requestId")
        val sourceId = value.getString("sourceId")
        assertNotEquals(oldAppId, record.id)
        assertEquals(requestId, value.getString("requestId"))
        assertEquals(record.id, value.getJSONObject("release").getString("appId"))
        assertTrue(isCanonicalUuidValue(sourceId))
        assertTrue(value.getJSONObject("record").getJSONArray("authoringSourceIds").strings().contains(sourceId))
        assertNotEquals(response(root, "create").getString("sourceId"), sourceId)

        val request = AuthoringRequest(requestId, "remix", record.name,
            "423e4567-e89b-12d3-a456-426614174000", "323e4567-e89b-12d3-a456-426614174000",
            oldAppId, revision = nativeRequest.getLong("revision"))
        val selection = AuthoringIntent(request.selectionId, sourceId, request.selectionId,
            value.getString("sourceDirectory"), request.projectId, request.workspaceId,
            authoringResultAppId(record, request.appId))
        val directory = Files.createTempDirectory("authoring-fixture-").toFile()
        try {
            val writer = ProjectAtomicWriter { target, text, validate -> validate(text); target.writeText(text) }
            val store = AuthoringIntentStore(directory, writer)
            store.saveRequest(request, SessionMode.Code)
            store.savePrepared(selection, SessionMode.Code)
            assertEquals(request, store.readRequest(requestId))
            assertEquals(selection, store.readSelection(requestId))
            assertEquals(oldAppId, store.readRequest(requestId)?.appId)
            assertEquals(record.id, store.readSelection(requestId)?.appId)
            assertEquals(sourceId, store.readSelection(requestId)?.sourceId)
            assertEquals(requestId, store.readSelection(requestId)?.sourceRequestId)
            assertNull(store.readSelection(requestId)?.sessionId)
            assertEquals(oldAppId, authoringResultAppId(null, oldAppId))
        } finally {
            directory.deleteRecursively()
        }
    }

    @Test fun marketMissingAndReplayDecodeWithoutInventingASource() {
        val root = fixture()
        val missing = root.getJSONObject("marketMissing").getJSONObject("response")
        val error = assertThrows(AppServiceException::class.java) { decodeAppResponse(missing.toString()) }
        assertEquals("archiveMissing", error.code)
        val imported = response(root, "marketImport")
        val replayed = response(root, "marketReplay")
        val request = root.getJSONObject("marketImport").getJSONObject("request")
        assertEquals(request.getString("requestId"), imported.getString("requestId"))
        assertEquals(normalized(imported), normalized(replayed))
        assertTrue(isCanonicalUuidValue(imported.getString("sourceId")))
        assertTrue(imported.getJSONObject("record").getJSONArray("authoringSourceIds").strings()
            .contains(imported.getString("sourceId")))
        assertEquals(imported.getJSONObject("record").appRecord().id,
            imported.getJSONObject("release").getString("appId"))
    }
}
