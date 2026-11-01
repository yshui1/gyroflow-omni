// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import "../components/"

MenuItem {
    id: languageMenu;
    property bool languageReady: false;
    text: "Language / 语言";
    iconName: "settings";
    opened: true;
    objectName: "language";

    Item {
        id: sett;
        property string lang: ui_tools.get_default_language().startsWith("zh") ? "zh_CN" : "en";
        Component.onCompleted: {
            // Retain the existing language preference key used by Advanced.
            settings.init(sett);
            lang = lang.startsWith("zh") ? "zh_CN" : "en";
            languageMenu.languageReady = true;
            applyLanguage();
        }
        function applyLanguage(): void {
            window.LayoutMirroring.enabled = false;
            window.LayoutMirroring.childrenInherit = true;
            ui_tools.set_language(lang);
        }
        onLangChanged: if (languageMenu.languageReady) applyLanguage();
        function propChanged() { settings.setValue("lang", lang); }
    }
    ComboBox {
        width: parent.width;
        model: ["English", "简体中文"];
        currentIndex: sett.lang === "zh_CN" ? 1 : 0;
        onActivated: index => sett.lang = index === 1 ? "zh_CN" : "en";
    }
}
