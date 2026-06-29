package io.github.xmoezzz.sena;

public enum NlsOption {
    SHIFT_JIS("ShiftJIS", "sjis"),
    GBK("GBK", "gbk"),
    UTF_8("UTF-8", "utf-8");

    public final String label;
    public final String value;

    NlsOption(String label, String value) {
        this.label = label;
        this.value = value;
    }

    public static NlsOption fromValue(String value) {
        if (value != null) {
            for (NlsOption option : values()) {
                if (option.value.equalsIgnoreCase(value) || option.label.equalsIgnoreCase(value)) {
                    return option;
                }
            }
        }
        return SHIFT_JIS;
    }

    public static String[] labels() {
        NlsOption[] options = values();
        String[] labels = new String[options.length];
        for (int i = 0; i < options.length; i++) {
            labels[i] = options[i].label;
        }
        return labels;
    }

    public static int indexOfValue(String value) {
        NlsOption option = fromValue(value);
        NlsOption[] options = values();
        for (int i = 0; i < options.length; i++) {
            if (options[i] == option) return i;
        }
        return 0;
    }
}
