package sh.zeron.android.voice.screen

import android.content.ComponentName
import android.content.Intent
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.*
import org.json.JSONObject
import org.junit.*
import org.junit.Assert.*
import org.junit.runner.RunWith
import sh.zeron.android.MainActivity
import sh.zeron.android.ZeronApplication
import java.net.HttpURLConnection
import java.net.URL

@RunWith(AndroidJUnit4::class)
class PhoneScreenTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private val context get() = instrumentation.targetContext
    private val automation get() = instrumentation.getUiAutomation(android.app.UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES)
    private fun shell(command: String) = android.os.ParcelFileDescriptor.AutoCloseInputStream(automation.executeShellCommand(command)).use { String(it.readBytes()) }
    private var previousServices = ""
    @Before fun setup() {
        previousServices = shell("settings get secure enabled_accessibility_services").trim()
        // A previous instrumentation process can die with a stale system
        // binding. Rebind the owned fixture service for each independent run.
        shell("settings put secure enabled_accessibility_services ''")
        shell("settings put secure accessibility_enabled 0")
        runBlocking { delay(200) }
        shell("settings put secure enabled_accessibility_services sh.zeron.android/.voice.screen.JarvisAccessibilityService")
        shell("settings put secure accessibility_enabled 1")
        runBlocking {
            withTimeout(10_000) { while (JarvisAccessibilityService.current == null) delay(50) }
        }
        fixture()
    }
    private fun fixture(secure: Boolean = false) {
        shell("am start -W -n ${instrumentation.context.packageName}/${ScreenFixtureActivity::class.java.name} -f 0x10008000 --ez secure $secure")
        runBlocking {
            withTimeout(5000) { while (withContext(Dispatchers.Main) { runCatching { service().observe().text.contains("Task pending") }.getOrDefault(false) }.not()) delay(50) }
            delay(750) // finish activity transitions before the live model benchmark
        }
    }
    private fun service() = JarvisAccessibilityService.current ?: error("Service unavailable")
    @After fun cleanup() {
        (context.applicationContext as ZeronApplication).existingModel?.jarvis?.screen?.stop()
        java.io.File(context.filesDir, "jev-test.key").delete()
        java.io.File(context.filesDir, "phone-planner-bridge.json").delete()
        val value = previousServices.takeUnless { it.isBlank() || it == "null" }.orEmpty()
        shell("settings put secure enabled_accessibility_services '$value'")
        if (value.isEmpty()) runBlocking { withTimeoutOrNull(3000) { while (JarvisAccessibilityService.current != null) delay(50) } }
    }
    @Test fun realControlsTypingCoordinateTapScrollingAndStaleRejection() = runBlocking<Unit> {
        withContext(Dispatchers.Main) {
            val screen = service().observe()
            assertFalse(screen.json().toString().contains("DO_NOT_SEND_PASSWORD"))
            val complete = screen.elements.firstOrNull { it.label.equals("Mark complete", true) }
                ?: error("Mark complete missing in fixture controls: ${screen.elements.map { it.label + it.operations }}")
            service().execute(screen, ScreenDecision("CLICK", complete.id, 1.0, 0), null)
            delay(100)
            service().observe().let { assertTrue("After click: ${it.text}", it.text.contains("Task completed")) }
            try { service().execute(screen, ScreenDecision("CLICK", complete.id, 1.0, 0), null); fail("Stale action accepted") }
            catch (_: IllegalArgumentException) { }
            val current = service().observe()
            val input = current.elements.firstOrNull { "TYPE" in it.operations } ?: error("Input missing in fixture: ${current.elements}")
            service().execute(current, ScreenDecision("TYPE", input.id, 1.0, 0), "hello jarvis")
            delay(100)
            assertTrue(service().observe().text.contains("hello jarvis"))
            // Hide the input method before testing physical screen coordinates.
            if (service().windows.any { it.type == android.view.accessibility.AccessibilityWindowInfo.TYPE_INPUT_METHOD }) {
                service().execute(service().observe(), ScreenDecision("BACK", null, 1.0, 0), null)
                delay(150)
            }
            val forTap = service().observe()
            val button = forTap.elements.firstOrNull { it.label.equals("Coordinate tap", true) } ?: error("Coordinate button missing in fixture: ${forTap.elements}")
            service().tap(forTap, (button.bounds[0]+button.bounds[2])/2.0, (button.bounds[1]+button.bounds[3])/2.0)
            delay(100)
            assertTrue(service().observe().text.contains("Coordinate tapped"))
            val scroll = service().observe()
            val container = scroll.elements.firstOrNull { "SCROLL_FORWARD" in it.operations } ?: error("Scroll container missing: ${scroll.elements}")
            service().execute(scroll, ScreenDecision("SCROLL_FORWARD", container.id, 1.0, 0), null)
            delay(300)
            assertNotEquals(scroll.fingerprint, service().observe().fingerprint)
            service().execute(service().observe(), ScreenDecision("HOME", null, 1.0, 0), null)
        }
    }
    @Test fun systemPanelOcclusionInvalidatesUnderlyingAppSelection() = runBlocking<Unit> {
        val before = withContext(Dispatchers.Main) { service().observe() }
        val button = before.elements.first { it.label.equals("Mark complete", true) }
        try {
            shell("cmd statusbar expand-notifications")
            withTimeout(3000) { while (withContext(Dispatchers.Main) { service().observe().packageName == before.packageName }) delay(50) }
            withContext(Dispatchers.Main) {
                try { service().execute(before, ScreenDecision("CLICK", button.id, 1.0, 0), null); fail("Covered app selection accepted") }
                catch (_: StaleScreenException) { }
            }
        } finally { shell("cmd statusbar collapse") }
    }
    @Test fun actualWindowScreenshotAndProtectedScreenFailure() = runBlocking<Unit> {
        withContext(Dispatchers.Main) {
            val bitmap = service().screenshot()
            assertTrue(bitmap.width > 500 && bitmap.height > 1000)
            java.io.File(context.filesDir, "jarvis-phone-screen-test.png").outputStream().use { bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG,100,it) }
            bitmap.recycle()
        }
        fixture(true)
        withContext(Dispatchers.Main) {
            try { service().screenshot().recycle(); fail("Protected window captured") } catch (_: IllegalStateException) { }
        }
    }
    @Test fun encryptedByokAndAuthenticatedCallBridge() = runBlocking<Unit> {
        val settings = ScreenSettings(context, "jarvis-screen-test")
        settings.saveKey("fixture-key-never-used-with-provider")
        assertEquals("fixture-key-never-used-with-provider", settings.readKey())
        val persisted = context.getSharedPreferences("jarvis-screen-test",0).getString("key", "")!!
        assertTrue(persisted.isNotEmpty())
        assertFalse(persisted.contains("fixture-key")); settings.clearKey(); assertNull(settings.readKey())
        val scope = CoroutineScope(SupervisorJob()+Dispatchers.Main)
        var active = true
        val bridge = PhoneControlBridge(scope, { active }) { JSONObject().put("status","fixture") }
        val instructions = bridge.instructions()
        val url = Regex("http://127.0.0.1:\\d+/v1/phone").find(instructions)!!.value
        val token = Regex("Authorization: Bearer ([0-9a-f]{64})").find(instructions)!!.groupValues[1]
        suspend fun request(auth: String?, origin: Boolean = false): Pair<Int,String> = withContext(Dispatchers.IO) {
            val connection = URL(url).openConnection() as HttpURLConnection
            connection.requestMethod="POST"; connection.doOutput=true; connection.connectTimeout=2000;connection.readTimeout=2000
            if (auth != null) connection.setRequestProperty("Authorization","Bearer $auth")
            if(origin) connection.setRequestProperty("Origin","https://fixture.invalid")
            val bytes="{\"operation\":\"state\"}".toByteArray();connection.setFixedLengthStreamingMode(bytes.size)
            connection.outputStream.use { it.write(bytes) }
            val status=connection.responseCode
            val body=(if(status==200)connection.inputStream else connection.errorStream).use { String(it.readBytes()) }
            connection.disconnect();status to body
        }
        try {
            assertEquals(401,request(null).first)
            assertEquals(400,request(token,true).first)
            assertEquals("fixture",JSONObject(request(token).second).getString("status"))
            active=false
            assertEquals("error",JSONObject(request(token).second).getString("status"))
        } finally { bridge.close();scope.cancel() }
    }
    @Test fun runtimeGptPlannerHandoff() = runBlocking<Unit> {
        Assume.assumeTrue(InstrumentationRegistry.getArguments().getString("jarvisPlannerHarness") == "1")
        val keyFile = java.io.File(context.filesDir, "jev-test.key")
        Assume.assumeTrue(keyFile.exists())
        val endpoint = java.io.File(context.filesDir, "phone-planner-bridge.json")
        withContext(Dispatchers.Main) { context.startActivity(Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK).putExtra("demo", true)) }
        val model = (context.applicationContext as ZeronApplication).model
        withContext(Dispatchers.Main) { model.startDemo() }
        withTimeout(30_000) { while (model.client.value == null) delay(50) }
        val controller = model.jarvis
        val screen = controller.screen
        val oldControl = screen.settings.controlEnabled.value
        val oldPrefs = context.getSharedPreferences("jarvis-screen", 0).all.filterKeys { it in listOf("key", "iv") }
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
        var bridge: PhoneControlBridge? = null
        try {
            withContext(Dispatchers.Main) {
                controller.stop(); screen.settings.saveKey(keyFile.readText().trim()); keyFile.delete(); screen.setControl(true)
                val generation = sh.zeron.android.voice.JarvisController::class.java.getDeclaredField("generation").apply { isAccessible = true }
                val id = (generation.get(controller) as sh.zeron.android.voice.VoiceGeneration).begin()
                @Suppress("UNCHECKED_CAST")
                val callState = sh.zeron.android.voice.JarvisController::class.java.getDeclaredField("_state").apply { isAccessible = true }
                    .get(controller) as kotlinx.coroutines.flow.MutableStateFlow<sh.zeron.android.voice.JarvisState>
                callState.value = sh.zeron.android.voice.JarvisState(live = true, call = uniffi.zeron_core.VoiceCallState(
                    uniffi.zeron_core.VoiceCallPhase.ACTIVE, uniffi.zeron_core.VoiceOrb.LISTENING, null, uniffi.zeron_core.VoiceCallWork.IDLE,
                    false, false, "", null, null, 0f, 0f, emptyList()))
                ScreenController::class.java.getDeclaredField("generation").apply { isAccessible = true }.set(screen, id)
                @Suppress("UNCHECKED_CAST")
                val controlState = ScreenController::class.java.getDeclaredField("_state").apply { isAccessible = true }.get(screen)
                    as kotlinx.coroutines.flow.MutableStateFlow<PhoneScreenState>
                controlState.value = PhoneScreenState(controlling = true)
                bridge = PhoneControlBridge(scope, screen::allowed, screen::handle)
                ScreenController::class.java.getDeclaredField("bridge").apply { isAccessible = true }.set(screen, bridge)
            }
            fixture()
            val instructions = bridge!!.instructions()
            val port = Regex("http://127.0.0.1:(\\d+)/v1/phone").find(instructions)!!.groupValues[1].toInt()
            val token = Regex("Authorization: Bearer ([0-9a-f]{64})").find(instructions)!!.groupValues[1]
            endpoint.writeText(JSONObject().put("port", port).put("token", token).toString())
            withTimeout(120_000) {
                while (withContext(Dispatchers.Main) {
                    val observed = service().observe()
                    !(observed.text.contains("Task completed") && observed.elements.any { it.value == "pizza" })
                }) delay(100)
            }
            android.util.Log.i("JarvisScreenValidation", "gpt_planner_bridge=true native_Jev_typing_and_click=true outcome_verified=true call_state=fixture")
            delay(2000) // let the last bridge response reach the external planner
        } finally {
            endpoint.delete(); keyFile.delete()
            withContext(Dispatchers.Main) { screen.stopControl(); screen.settings.clearKey(); screen.setControl(oldControl); controller.stop() }
            context.getSharedPreferences("jarvis-screen", 0).edit().also { editor -> oldPrefs.forEach { (k,v) -> editor.putString(k,v as String) } }.commit()
            bridge?.close(); scope.cancel()
        }
    }
    @Test fun liveJevGroundsAndExecutesActualAndroidButton() = runBlocking<Unit> {
        // Optional credential is provisioned privately at runtime, never in
        // either APK or source. Offline runs skip only this provider check.
        val keyFile=java.io.File(context.filesDir,"jev-test.key")
        Assume.assumeTrue(keyFile.exists())
        val key=keyFile.readText().trim()
        try {
            val observed=withContext(Dispatchers.Main) { service().observe() }
            val decision=JevClient { key }.decide(observed,"Press Mark complete so Task pending becomes Task completed",null,emptyList())
            assertEquals("CLICK",decision.operation)
            assertTrue(observed.elements.first { it.id == decision.target }.label.equals("Mark complete", true))
            withContext(Dispatchers.Main) { service().execute(observed,decision,null) }
            withTimeout(3000) { while (withContext(Dispatchers.Main) { !service().observe().text.contains("Task completed") }) delay(50) }
            android.util.Log.i("JarvisScreenValidation","live Jev decision_ms=${decision.latencyMs} verified=TaskCompleted")
        } finally { keyFile.delete() }
    }
}
