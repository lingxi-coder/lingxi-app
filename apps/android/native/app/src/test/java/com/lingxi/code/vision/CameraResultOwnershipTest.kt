package com.lingxi.code.vision

import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.async
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Test

class CameraResultOwnershipTest {
    private class Host {
        val owner = Any()
        var granted = true
        var permissionLaunches = 0
        var pictureLaunches = 0
        var mediaLaunches = 0
        fun value() = CameraResultHost(owner, { granted }, { permissionLaunches++ }, { pictureLaunches++ }, { mediaLaunches++ })
    }
    private class Harness {
        val posts = ArrayDeque<() -> Unit>()
        val controller = CameraResultCoordinator { posts.addLast(it) }
        fun step() = posts.removeFirst().invoke()
        fun picture(owner: Any, width: Int) = controller.onResult(owner, CameraResultCoordinator.ResultKind.Picture) { CapturedImage(byteArrayOf(1), width, 10) }
    }

    @Test fun cancelledExternalCaptureKeepsItsResultSlotUntilDrained() = runBlocking {
        val h = Harness(); val host = Host(); h.controller.attach(host.value())
        val first = async(start = CoroutineStart.UNDISPATCHED) { h.controller.capturePhoto() }; h.step(); first.cancelAndJoin()
        val blocked = async(start = CoroutineStart.UNDISPATCHED) { runCatching { h.controller.capturePhoto() } }
        assertTrue(blocked.await().exceptionOrNull() is CameraException); assertEquals(1, host.pictureLaunches)
        var decodedOldImage = false
        h.controller.onResult(host.owner, CameraResultCoordinator.ResultKind.Picture) { decodedOldImage = true; CapturedImage(byteArrayOf(1), 11, 10) }
        assertFalse(decodedOldImage)
        val next = async(start = CoroutineStart.UNDISPATCHED) { h.controller.capturePhoto() }; h.step(); h.picture(host.owner, 22)
        assertEquals(22, next.await().width); assertEquals(2, host.pictureLaunches)
    }

    @Test fun concurrentCaptureDoesNotReplaceTheActiveRequest() = runBlocking {
        val h = Harness(); val host = Host(); h.controller.attach(host.value())
        val first = async(start = CoroutineStart.UNDISPATCHED) { h.controller.capturePhoto() }; h.step()
        val second = async(start = CoroutineStart.UNDISPATCHED) { runCatching { h.controller.pickFromLibrary() } }
        assertTrue(second.await().isFailure); assertFalse(first.isCompleted); assertEquals(0, host.mediaLaunches)
        h.picture(host.owner, 11); assertEquals(11, first.await().width)
    }

    @Test fun oldActivityResultAndDetachCannotSettleNewActivityCapture() = runBlocking {
        val h = Harness(); val old = Host(); h.controller.attach(old.value())
        val oldRequest = async(start = CoroutineStart.UNDISPATCHED) { runCatching { h.controller.capturePhoto() } }; h.step()
        val nextHost = Host(); h.controller.attach(nextHost.value()); assertTrue(oldRequest.await().isFailure)
        val next = async(start = CoroutineStart.UNDISPATCHED) { h.controller.capturePhoto() }; h.step()
        h.picture(old.owner, 11); h.controller.detach(old.owner); assertFalse(next.isCompleted)
        h.picture(nextHost.owner, 22); assertEquals(22, next.await().width)
    }

    @Test fun cancellationBeforePostedLaunchDoesNotOpenAnOldCameraUi() = runBlocking {
        val h = Harness(); val host = Host(); h.controller.attach(host.value())
        val first = async(start = CoroutineStart.UNDISPATCHED) { h.controller.capturePhoto() }; first.cancelAndJoin()
        val next = async(start = CoroutineStart.UNDISPATCHED) { h.controller.capturePhoto() }
        h.step(); assertEquals(0, host.pictureLaunches); h.step(); assertEquals(1, host.pictureLaunches)
        h.picture(host.owner, 22); assertEquals(22, next.await().width)
    }

    @Test fun cancelledPermissionResultDrainsWithoutLaunchingAPhoto() = runBlocking {
        val h = Harness(); val host = Host(); host.granted = false; h.controller.attach(host.value())
        val first = async(start = CoroutineStart.UNDISPATCHED) { h.controller.capturePhoto() }; h.step(); first.cancelAndJoin()
        val blocked = async(start = CoroutineStart.UNDISPATCHED) { runCatching { h.controller.capturePhoto() } }; assertTrue(blocked.await().isFailure)
        h.controller.onCameraPermission(host.owner, true); assertEquals(0, host.pictureLaunches); assertTrue(h.posts.isEmpty())
        host.granted = true; val next = async(start = CoroutineStart.UNDISPATCHED) { h.controller.capturePhoto() }; h.step()
        h.controller.onCameraPermission(host.owner, false); assertFalse(next.isCompleted)
        h.picture(host.owner, 22); assertEquals(22, next.await().width)
    }

    @Test fun wrongResultContractAndStalePickerPayloadAreNotDecoded() = runBlocking {
        val h = Harness(); val old = Host(); h.controller.attach(old.value())
        val oldRequest = async(start = CoroutineStart.UNDISPATCHED) { h.controller.pickFromLibrary() }; h.step(); oldRequest.cancelAndJoin()
        var decoded = false
        h.picture(old.owner, 11)
        val blocked = async(start = CoroutineStart.UNDISPATCHED) { runCatching { h.controller.capturePhoto() } }; assertTrue(blocked.await().isFailure)
        h.controller.onResult(old.owner, CameraResultCoordinator.ResultKind.Media) { decoded = true; CapturedImage(byteArrayOf(1), 11, 10) }
        assertFalse(decoded)
        val nextHost = Host(); h.controller.attach(nextHost.value())
        val next = async(start = CoroutineStart.UNDISPATCHED) { h.controller.pickFromLibrary() }; h.step()
        h.controller.onResult(old.owner, CameraResultCoordinator.ResultKind.Media) { decoded = true; CapturedImage(byteArrayOf(1), 11, 10) }; assertFalse(decoded)
        h.controller.onResult(nextHost.owner, CameraResultCoordinator.ResultKind.Media) { CapturedImage(byteArrayOf(2), 22, 10) }
        assertEquals(22, next.await().width)
    }
}
