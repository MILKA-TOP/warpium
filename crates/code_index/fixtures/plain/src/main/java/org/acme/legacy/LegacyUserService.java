package org.acme.legacy;

import java.util.List;
import static java.util.Collections.emptyList;

public class LegacyUserService {
    public static final int MAX_USERS = 100;
    private String cacheKey;

    LegacyUserService() {}

    public List<String> userNames() {
        Runnable anonymous = new Runnable() {
            public void run() {}
        };
        return emptyList();
    }

    protected static class Cache {}

    interface Callback {
        void onLoaded();
    }

    enum Mode { FAST, SAFE }

    record Snapshot(int count) {}
}
