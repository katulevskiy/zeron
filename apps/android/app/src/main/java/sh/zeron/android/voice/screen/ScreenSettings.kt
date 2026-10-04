package sh.zeron.android.voice.screen

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Private BYOK, encrypted with a non-exportable Android Keystore key. Nothing
 * is passed to Codex, model prompts, logs, build flags, or the guest filesystem. */
class ScreenSettings(context: Context, preferencesName: String = "jarvis-screen") {
    private val prefs = context.getSharedPreferences(preferencesName, Context.MODE_PRIVATE)
    private val _contextEnabled = MutableStateFlow(prefs.getBoolean("context", false))
    val contextEnabled = _contextEnabled.asStateFlow()
    private val _controlEnabled = MutableStateFlow(prefs.getBoolean("control", false))
    val controlEnabled = _controlEnabled.asStateFlow()
    private val _hasKey = MutableStateFlow(prefs.contains("key"))
    val hasKey = _hasKey.asStateFlow()
    fun setContext(enabled: Boolean) { prefs.edit().putBoolean("context", enabled).apply(); _contextEnabled.value = enabled }
    fun setControl(enabled: Boolean) { prefs.edit().putBoolean("control", enabled).apply(); _controlEnabled.value = enabled }
    private fun key(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(ALIAS, null) as? SecretKey)?.let { return it }
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
            init(KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).build())
        }.generateKey()
    }
    fun saveKey(value: String) {
        val secret = value.trim(); require(secret.length in 8..512 && secret.none { it.isWhitespace() })
        val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.ENCRYPT_MODE, key()) }
        val bytes = cipher.doFinal(secret.toByteArray())
        check(prefs.edit().putString("key", Base64.encodeToString(bytes, Base64.NO_WRAP))
            .putString("iv", Base64.encodeToString(cipher.iv, Base64.NO_WRAP)).commit())
        _hasKey.value = true
    }
    internal fun readKey(): String? {
        val value = prefs.getString("key", null) ?: return null
        return try {
            val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply {
                init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, Base64.decode(prefs.getString("iv", ""), Base64.NO_WRAP)))
            }
            String(cipher.doFinal(Base64.decode(value, Base64.NO_WRAP)), Charsets.UTF_8)
        } catch (_: Exception) { null }
    }
    fun clearKey() { prefs.edit().remove("key").remove("iv").apply(); _hasKey.value = false }
    companion object { private const val ALIAS = "zeron.jarvis.jev.v1" }
}
