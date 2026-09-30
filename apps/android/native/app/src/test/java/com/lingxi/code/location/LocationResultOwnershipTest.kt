package com.lingxi.code.location

import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.async
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Test

class LocationResultOwnershipTest {
    private class Host {
        val owner = Any()
        var granted = true
        var permissionLaunches = 0
        val callbacks = mutableListOf<(Result<DeviceLocationFix>) -> Unit>()
        val cancellations = mutableListOf<Int>()
        fun value() = LocationResultHost(owner, { granted }, { true }, { permissionLaunches++ }) { callback ->
            val index = callbacks.size; callbacks.add(callback); { cancellations.add(index); Unit }
        }
    }
    private class Harness {
        val posts = ArrayDeque<() -> Unit>()
        val timers = linkedSetOf<Runnable>()
        val controller = LocationResultCoordinator({ posts.addLast(it) }, { run, _ -> timers.add(run); Unit }, { timers.remove(it); Unit })
        fun step() = posts.removeFirst().invoke()
        fun fix(latitude: Double) = DeviceLocationFix(latitude, 121.47, 1.0, 1L)
    }

    @Test fun lateSuccessErrorAndTimeoutFromACannotSettleB() = runBlocking {
        val h = Harness(); val host = Host(); h.controller.attach(host.value())
        val first = async(start = CoroutineStart.UNDISPATCHED) { h.controller.currentLocation() }; h.step()
        val oldTimeout = h.timers.single(); first.cancelAndJoin(); assertEquals(listOf(0), host.cancellations)
        val next = async(start = CoroutineStart.UNDISPATCHED) { h.controller.currentLocation() }; h.step()
        val newTimeout = h.timers.single()
        host.callbacks[0](Result.success(h.fix(11.0))); host.callbacks[0](Result.failure(LocationException(LocationFailure.Unavailable))); oldTimeout.run()
        assertFalse(next.isCompleted); assertTrue(h.timers.contains(newTimeout)); assertEquals(listOf(0), host.cancellations)
        host.callbacks[1](Result.success(h.fix(22.0))); assertEquals(22.0, next.await().latitude, 0.0)
        assertEquals(listOf(0, 1), host.cancellations); assertTrue(h.timers.isEmpty())
    }

    @Test fun cancelledPermissionDialogKeepsItsResultTombstone() = runBlocking {
        val h = Harness(); val host = Host(); host.granted = false; h.controller.attach(host.value())
        val first = async(start = CoroutineStart.UNDISPATCHED) { h.controller.currentLocation() }; h.step(); first.cancelAndJoin()
        val blocked = async(start = CoroutineStart.UNDISPATCHED) { runCatching { h.controller.currentLocation() } }
        assertTrue(blocked.await().isFailure); assertEquals(1, host.permissionLaunches)
        h.controller.onPermissionResult(host.owner, false)
        val next = async(start = CoroutineStart.UNDISPATCHED) { h.controller.currentLocation() }; h.step(); assertEquals(2, host.permissionLaunches)
        host.granted = true; h.controller.onPermissionResult(host.owner, true); host.callbacks.single()(Result.success(h.fix(22.0)))
        assertEquals(22.0, next.await().latitude, 0.0)
    }

    @Test fun oldActivityPermissionResultAndDetachCannotAffectNewHost() = runBlocking {
        val h = Harness(); val old = Host(); old.granted = false; h.controller.attach(old.value())
        val first = async(start = CoroutineStart.UNDISPATCHED) { runCatching { h.controller.currentLocation() } }; h.step()
        val host = Host(); host.granted = false; h.controller.attach(host.value()); assertTrue(first.await().isFailure)
        val next = async(start = CoroutineStart.UNDISPATCHED) { h.controller.currentLocation() }; h.step()
        h.controller.onPermissionResult(old.owner, true); h.controller.detach(old.owner)
        assertFalse(next.isCompleted); assertTrue(host.callbacks.isEmpty())
        host.granted = true; h.controller.onPermissionResult(host.owner, true); host.callbacks.single()(Result.success(h.fix(22.0)))
        assertEquals(22.0, next.await().latitude, 0.0)
    }

    @Test fun cancellationBeforePostedAdmissionDoesNotRequestNativeWork() = runBlocking {
        val h = Harness(); val host = Host(); h.controller.attach(host.value())
        val first = async(start = CoroutineStart.UNDISPATCHED) { h.controller.currentLocation() }; first.cancelAndJoin()
        val next = async(start = CoroutineStart.UNDISPATCHED) { h.controller.currentLocation() }
        h.step(); assertTrue(host.callbacks.isEmpty()); h.step(); assertEquals(1, host.callbacks.size)
        host.callbacks.single()(Result.success(h.fix(22.0))); assertEquals(22.0, next.await().latitude, 0.0)
    }

    @Test fun synchronousProviderCompletionClosesItsReturnedCancellationHandleOnce() = runBlocking {
        val h = Harness(); val owner = Any(); var cancels = 0
        h.controller.attach(LocationResultHost(owner, { true }, { true }, {}) { callback ->
            callback(Result.success(h.fix(22.0))); { cancels++ }
        })
        val request = async(start = CoroutineStart.UNDISPATCHED) { h.controller.currentLocation() }; h.step()
        assertEquals(22.0, request.await().latitude, 0.0); assertEquals(1, cancels); assertTrue(h.timers.isEmpty())
    }

    @Test fun timeoutEndsOnlyItsCapturedRequestAndRetainsOutstandingPermissionSlot() = runBlocking {
        val h = Harness(); val host = Host(); host.granted = false; h.controller.attach(host.value())
        val first = async(start = CoroutineStart.UNDISPATCHED) { runCatching { h.controller.currentLocation() } }; h.step(); h.timers.single().run()
        assertEquals(LocationFailure.Timeout, (first.await().exceptionOrNull() as LocationException).failure)
        val blocked = async(start = CoroutineStart.UNDISPATCHED) { runCatching { h.controller.currentLocation() } }; assertTrue(blocked.await().isFailure)
        h.controller.onPermissionResult(host.owner, false)
        host.granted = true; val next = async(start = CoroutineStart.UNDISPATCHED) { h.controller.currentLocation() }; h.step()
        host.callbacks.single()(Result.success(h.fix(22.0))); assertEquals(22.0, next.await().latitude, 0.0)
    }
}
