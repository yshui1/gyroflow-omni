// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <QFontDatabase>
#include <QGuiApplication>

inline QString omniUiFontFamily(const QString &language) {
    if (!language.startsWith(QLatin1String("zh"))) return QStringLiteral("Arial");
    const bool traditional = language.contains(QLatin1String("TW")) || language.contains(QLatin1String("HK"));
    QStringList candidates;
#if defined(Q_OS_WIN)
    candidates = traditional
        ? QStringList{"Microsoft JhengHei UI", "Microsoft JhengHei", "Microsoft YaHei UI", "Microsoft YaHei"}
        : QStringList{"Microsoft YaHei UI", "Microsoft YaHei", "DengXian", "Microsoft JhengHei UI"};
#elif defined(Q_OS_MACOS) || defined(Q_OS_IOS)
    candidates = traditional ? QStringList{"PingFang TC", "PingFang HK", "PingFang SC"}
                             : QStringList{"PingFang SC", "PingFang TC"};
#endif
    candidates += traditional
        ? QStringList{"Noto Sans CJK TC", "Noto Sans TC", "Source Han Sans TC", "Noto Sans CJK SC"}
        : QStringList{"Noto Sans CJK SC", "Noto Sans SC", "Source Han Sans SC", "Noto Sans CJK TC"};
    const QStringList installed = QFontDatabase::families();
    for (const auto &family : candidates)
        if (installed.contains(family, Qt::CaseInsensitive)) return family;
    return QFontDatabase::systemFont(QFontDatabase::GeneralFont).family();
}

inline QString omniApplyUiFont(const QString &language) {
    const QString family = omniUiFontFamily(language);
    auto font = QGuiApplication::font();
    font.setFamily(family);
    font.setStyleHint(QFont::SansSerif);
    QGuiApplication::setFont(font);
    return family;
}
