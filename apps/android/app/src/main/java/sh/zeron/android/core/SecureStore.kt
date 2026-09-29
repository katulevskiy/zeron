package sh.zeron.android.core

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import org.json.JSONObject
import uniffi.zeron_core.AuthTokens
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Secrets (WorkOS tokens, account profile) encrypted with an AES-GCM key that
 * never leaves the Android Keystore; the ciphertext sits in private prefs.
 * (EncryptedSharedPreferences does the same but is deprecated.)
 */
class SecureStore(context: Context) {
    private val prefs = context.getSharedPreferences("zeron.secure", Context.MODE_PRIVATE)
    private val alias = "zeron.credentials"

    private fun key(): SecretKey {
        val ks = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (ks.getKey(alias, null) as? SecretKey)?.let { return it }
        val gen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        gen.init(
            KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build(),
        )
        return gen.generateKey()
    }

    private fun put(name: String, plain: String?) {
        if (plain == null) {
            prefs.edit().remove(name).apply()
            return
        }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val sealed = cipher.iv + cipher.doFinal(plain.toByteArray())
        prefs.edit().putString(name, Base64.encodeToString(sealed, Base64.NO_WRAP)).apply()
    }

    private fun get(name: String): String? {
        val sealed = prefs.getString(name, null)?.let { Base64.decode(it, Base64.NO_WRAP) } ?: return null
        return runCatching {
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, sealed, 0, 12))
            String(cipher.doFinal(sealed, 12, sealed.size - 12))
        }.getOrNull()
    }

    /** A WorkOS session scoped to one org. */
    data class Account(val userId: String, val orgId: String, val tokens: AuthTokens)

    var account: Account?
        get() = get("account")?.let { raw ->
            runCatching {
                val o = JSONObject(raw)
                Account(o.getString("userId"), o.getString("orgId"), AuthTokens(o.getString("access"), o.getString("refresh")))
            }.getOrNull()
        }
        set(v) = put(
            "account",
            v?.let {
                JSONObject()
                    .put("userId", it.userId)
                    .put("orgId", it.orgId)
                    .put("access", it.tokens.accessToken)
                    .put("refresh", it.tokens.refreshToken)
                    .toString()
            },
        )

    /** Rotated pair (`AuthRefreshed`): the old refresh token is spent — persist now. */
    fun updateTokens(tokens: AuthTokens) {
        account?.let { account = it.copy(tokens = tokens) }
    }

    /** Display identity (the client itself only knows ids). */
    data class Profile(val name: String?, val email: String?, val orgName: String?)

    var profile: Profile
        get() = get("profile")?.let { raw ->
            runCatching {
                val o = JSONObject(raw)
                Profile(o.optString("name").ifEmpty { null }, o.optString("email").ifEmpty { null }, o.optString("orgName").ifEmpty { null })
            }.getOrNull()
        } ?: Profile(null, null, null)
        set(v) = put("profile", JSONObject().put("name", v.name ?: "").put("email", v.email ?: "").put("orgName", v.orgName ?: "").toString())

    fun clearAccount() {
        put("account", null)
        put("profile", null)
    }
}
