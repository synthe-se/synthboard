package app.synthboard

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Encrypts what the keyboard learned, on top of Android's file encryption. The
 * key lives in the phone's secure hardware and cannot be copied out, so a stolen
 * file alone is useless. Nothing is ever written in the clear if this fails.
 */
class Vault(private val key: () -> SecretKey) {
    fun seal(text: String): ByteArray {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        return cipher.iv + cipher.doFinal(text.toByteArray())
    }

    fun open(sealed: ByteArray): String {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        val iv = sealed.copyOfRange(0, IV_BYTES)
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(TAG_BITS, iv))
        return String(cipher.doFinal(sealed, IV_BYTES, sealed.size - IV_BYTES))
    }

    companion object {
        private const val TRANSFORMATION = "AES/GCM/NoPadding"
        private const val IV_BYTES = 12
        private const val TAG_BITS = 128
        private const val KEY_BITS = 256
        private const val STORE = "AndroidKeyStore"
        private const val ALIAS = "memory"

        /** The vault of this phone, with its key made on first use. */
        fun ofDevice() = Vault(::deviceKey)

        private fun deviceKey(): SecretKey {
            val store = KeyStore.getInstance(STORE).apply { load(null) }
            (store.getKey(ALIAS, null) as? SecretKey)?.let { return it }
            val spec = KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(KEY_BITS)
                .build()
            return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, STORE).run {
                init(spec)
                generateKey()
            }
        }
    }
}
