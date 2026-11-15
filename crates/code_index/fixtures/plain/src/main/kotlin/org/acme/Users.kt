@file:JvmName("Users")

package org.acme

import org.acme.legacy.LegacyUserService as Legacy
import java.util.UUID

interface UserRepository {
    fun findUser(id: String): User?
}

data class User(val id: String, var displayName: String)

sealed class LoadState {
    object Loading : LoadState()
    data class Loaded(val user: User) : LoadState()
}

class UserViewModel(private val repository: UserRepository) {
    companion object {
        fun create(): UserViewModel = TODO()
    }

    inner class Session

    protected open fun onCleared() {}

    constructor() : this(InMemoryUserRepository)
}

object InMemoryUserRepository : UserRepository {
    override fun findUser(id: String): User? = null
}

enum class Role { ADMIN, GUEST }

typealias UserId = String

private val defaultId: UserId = UUID.randomUUID().toString()

fun User.initials(): String = displayName.take(2)

val User.isAnonymous: Boolean get() = id.isEmpty()

fun interface UserListener {
    fun onUser(user: User)
}
