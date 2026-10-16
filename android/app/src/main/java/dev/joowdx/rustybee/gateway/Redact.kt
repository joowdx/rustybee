package dev.joowdx.rustybee.gateway

/** Log redaction: phone numbers keep only their last 4 digits, whether written with spaces, dashes, dots or parentheses (AC-011-07). */
object Redact {
    private val number = Regex("""(?<![A-Za-z0-9])\+?\(?\d[\d .()-]{3,}\d(?![A-Za-z0-9])""")

    fun phone(value: String): String {
        val digits = value.filter { it.isDigit() }
        return if (digits.length <= 4) value else "…" + digits.takeLast(4)
    }

    fun text(message: String): String = number.replace(message) { phone(it.value) }
}
