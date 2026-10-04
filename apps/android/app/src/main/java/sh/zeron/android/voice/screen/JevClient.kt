package sh.zeron.android.voice.screen

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.net.URL
import javax.net.ssl.HttpsURLConnection
import kotlin.coroutines.coroutineContext

internal class JevClient(private val key: () -> String?) {
    suspend fun decide(screen: ScreenSnapshot, goal: String, text: String?, history: List<String>): ScreenDecision {
        val request = ScreenPolicy.request(screen, goal, text, history)
        val started = android.os.SystemClock.elapsedRealtime()
        val response = evaluate(request)
        coroutineContext.ensureActive()
        return try { ScreenPolicy.decode(request, response, android.os.SystemClock.elapsedRealtime() - started) }
        catch (e: IllegalArgumentException) { throw e }
        catch (_: Exception) { error("Invalid Jev response; no action executed") }
    }
    suspend fun check(): Long {
        val started = android.os.SystemClock.elapsedRealtime()
        val request = JSONObject().put("model", "jev-latest").put("state", "Key connection check")
            .put("questions", JSONObject().put("connected", JSONObject().put("type", "noul").put("instructions", "Is this a connection check?")))
        require(evaluate(request).getJSONObject("answers").getJSONObject("connected").getDouble("noul").isFinite())
        return android.os.SystemClock.elapsedRealtime() - started
    }
    private suspend fun evaluate(request: JSONObject): JSONObject = withContext(Dispatchers.IO) {
        val secret = key()?.takeIf { it.isNotBlank() } ?: error("Add your Jev key in Jarvis settings")
        val connection = URL("https://api.typesafe.ai/v1/systemone").openConnection() as HttpsURLConnection
        connection.connectTimeout = 8000; connection.readTimeout = 12000
        connection.requestMethod = "POST"; connection.doOutput = true
        connection.setRequestProperty("Authorization", "Bearer $secret")
        connection.setRequestProperty("Content-Type", "application/json")
        val bytes = request.toString().toByteArray()
        connection.setFixedLengthStreamingMode(bytes.size)
        try {
            connection.outputStream.use { it.write(bytes) }
            val status = connection.responseCode
            if (status != 200) {
                connection.errorStream?.close()
                error("Jev returned HTTP $status; no action executed")
            }
            val body = connection.inputStream.use { input ->
                val result = java.io.ByteArrayOutputStream()
                val buffer = ByteArray(4096)
                while (result.size() <= 256_000) {
                    val count = input.read(buffer, 0, minOf(buffer.size, 256_001 - result.size()))
                    if (count <= 0) break
                    result.write(buffer, 0, count)
                }
                result.toByteArray()
            }
            require(body.size <= 256_000) { "Oversized Jev response" }
            coroutineContext.ensureActive()
            JSONObject(String(body, Charsets.UTF_8))
        } catch (e: kotlinx.coroutines.CancellationException) { connection.disconnect(); throw e }
        catch (e: IllegalStateException) { connection.disconnect(); throw e }
        catch (e: IllegalArgumentException) { connection.disconnect(); throw e }
        catch (_: Exception) { connection.disconnect(); error("Jev connection failed; no action executed") }
    }
}
