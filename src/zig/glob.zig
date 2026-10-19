//! Validate policy patterns before any sync mutations.
pub fn validate(pattern: []const u8) error{InvalidGlobPattern}!void {
    if (pattern.len > 4096) return error.InvalidGlobPattern;
    var braces: usize = 0;
    var i: usize = 0;
    while (i < pattern.len) : (i += 1) {
        switch (pattern[i]) {
            '\\' => {
                i += 1;
                if (i == pattern.len) return error.InvalidGlobPattern;
            },
            '{' => {
                braces += 1;
                if (braces > 16) return error.InvalidGlobPattern;
            },
            '}' => {
                if (braces == 0) return error.InvalidGlobPattern;
                braces -= 1;
            },
            '[' => {
                i += 1;
                if (i < pattern.len and (pattern[i] == '!' or pattern[i] == '^')) i += 1;
                const start = i;
                while (i < pattern.len and pattern[i] != ']') : (i += 1) {}
                if (i == pattern.len or i == start) return error.InvalidGlobPattern;
            },
            else => {},
        }
    }
    if (braces != 0) return error.InvalidGlobPattern;
}
