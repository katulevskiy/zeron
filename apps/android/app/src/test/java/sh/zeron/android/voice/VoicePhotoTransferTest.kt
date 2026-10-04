package sh.zeron.android.voice

import kotlinx.coroutines.*
import org.junit.Assert.*
import org.junit.Test
import uniffi.zeron_core.BusyPolicy

class VoicePhotoTransferTest {
    @Test fun photoUsesCanonicalAttachmentAndQueuesWithoutInterruptingWork() {
        val bytes = byteArrayOf(1, 2, 3)
        val request = voicePhotoRequest(bytes, "camera.jpg")
        assertEquals(BusyPolicy.QUEUE, request.busy)
        assertNull(request.worktree)
        val image = request.attachments.single()
        assertEquals("image/jpeg", image.mimeType)
        assertEquals("camera.jpg", image.name)
        assertArrayEquals(bytes, image.data)
        assertTrue(request.text.contains("untrusted context"))
        assertTrue(request.text.contains("Open the attached image with the image viewer"))
        assertTrue(request.text.contains("use other tools"))
        assertTrue(request.text.contains("Do not perform actions"))
    }

    @Test fun localSubmissionWaitsForTheExactHostAdoption() = runBlocking {
        var submitted = 0
        var adopted = false
        val sender = VoicePhotoTransfer({ true }, { submitted++; VoicePhotoReceipt("photo-1", false) }, {
            assertEquals("photo-1", it.id)
            if (adopted) VoicePhotoProgress.ADOPTED else VoicePhotoProgress.PENDING
        })
        val result = async { sender.add(byteArrayOf(1)) }
        yield()
        assertEquals(1, submitted)
        assertFalse(result.isCompleted)
        adopted = true
        assertEquals("Photo added", result.await())
        assertEquals(1, submitted)
    }

    @Test fun queuedPhotoDoesNotWaitForOrInterruptExistingTurn() = runBlocking {
        val sender = VoicePhotoTransfer({ true }, { VoicePhotoReceipt("queue-1", true) }, { error("Must preserve the existing turn") })
        assertEquals("Photo queued", sender.add(byteArrayOf(1)))
    }

    @Test fun replacementCallCannotSubmitOrReportSuccess() = runBlocking {
        var live = false
        var sent = 0
        val sender = VoicePhotoTransfer({ live }, { sent++; live = false; VoicePhotoReceipt("photo-1", false) }, { VoicePhotoProgress.ADOPTED })
        assertEquals(VoicePhotoException.Reason.ENDED, (runCatching { sender.add(byteArrayOf(1)) }.exceptionOrNull() as VoicePhotoException).reason)
        assertEquals(0, sent)
        live = true
        assertEquals(VoicePhotoException.Reason.ENDED, (runCatching { sender.add(byteArrayOf(1)) }.exceptionOrNull() as VoicePhotoException).reason)
        assertEquals(1, sent)
    }

    @Test fun uploadFailureDoesNotClaimSuccess() = runBlocking {
        val sender = VoicePhotoTransfer({ true }, { VoicePhotoReceipt("photo-1", false) }, { VoicePhotoProgress.FAILED })
        assertEquals(VoicePhotoException.Reason.TRANSFER, (runCatching { sender.add(byteArrayOf(1)) }.exceptionOrNull() as VoicePhotoException).reason)
    }

    @Test fun timeoutDoesNotResubmitOrClaimSuccess() = runBlocking {
        var submitted = 0
        val sender = VoicePhotoTransfer({ true }, { submitted++; VoicePhotoReceipt("photo-1", false) }, { VoicePhotoProgress.PENDING }, 20)
        assertTrue(sender.add(byteArrayOf(1)).startsWith("Photo pending"))
        assertEquals(1, submitted)
    }

    @Test fun cancellationCannotClaimAdoptionOrResubmit() = runBlocking {
        var submitted = 0
        val sender = VoicePhotoTransfer({ true }, { submitted++; VoicePhotoReceipt("photo-1", false) }, { VoicePhotoProgress.PENDING })
        val result = async { sender.add(byteArrayOf(1)) }
        yield()
        result.cancelAndJoin()
        assertTrue(result.isCancelled)
        assertEquals(1, submitted)
    }
}
