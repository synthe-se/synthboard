package se.synthe.synthboard

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Guards the secret key that opens what the keyboard learned. The key that seals
 * it lives in the phone's secure hardware and cannot be copied out, and it only
 * works while the phone is unlocked, so stolen files alone are useless.
 */
class Vault(private val key: () -> SecretKey) {
    fun seal(secret: ByteArray): ByteArray {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        cipher.updateAAD(FORMAT)
        return cipher.iv + cipher.doFinal(secret)
    }

    fun open(sealed: ByteArray): ByteArray {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        val iv = sealed.copyOfRange(0, IV_BYTES)
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(TAG_BITS, iv))
        cipher.updateAAD(FORMAT)
        return cipher.doFinal(sealed, IV_BYTES, sealed.size - IV_BYTES)
    }

    companion object {
        private const val TRANSFORMATION = "AES/GCM/NoPadding"
        private const val IV_BYTES = 12
        private const val TAG_BITS = 128
        private const val KEY_BITS = 256
        private const val STORE = "AndroidKeyStore"
        private const val ALIAS = "memory.unlocked"
        private val FORMAT = "synthboard memory v2".toByteArray()

        /** The vault of this phone, with its key made on first use. */
        fun ofDevice() = Vault(::deviceKey)

        private fun store() = KeyStore.getInstance(STORE).apply { load(null) }

        /** Destroys the key, which is what truly erases: flash keeps deleted files. */
        fun forget() = store().deleteEntry(ALIAS)

        private fun deviceKey(): SecretKey {
            (store().getKey(ALIAS, null) as? SecretKey)?.let { return it }
            val spec = KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(KEY_BITS)
                .setUnlockedDeviceRequired(true)
                .build()
            return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, STORE).run {
                init(spec)
                generateKey()
            }
        }
    }
}
