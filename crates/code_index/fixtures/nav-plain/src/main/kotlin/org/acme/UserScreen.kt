package org.acme

import org.acme.legacy.LegacyUserService as Legacy

class UserScreen {
    private val service = Legacy()

    fun show(id: String): String {
        val user = service.find(id) ?: return ""
        val max = Legacy.MAX_USERS
        return user.initials() + user.name + max
    }
}
