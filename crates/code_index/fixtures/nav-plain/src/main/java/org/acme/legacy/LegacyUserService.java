package org.acme.legacy;

import org.acme.User;

public class LegacyUserService {
    public static final int MAX_USERS = 100;
    private final Cache cache = new Cache();

    public User find(String id) {
        User user = cache.lookup(id);
        return user;
    }

    static class Cache {
        User lookup(String id) {
            return null;
        }
    }
}
