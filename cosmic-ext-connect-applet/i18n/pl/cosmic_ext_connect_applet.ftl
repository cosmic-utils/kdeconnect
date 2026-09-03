# Applet header
applet-title = Cosmic Ext Connect
applet-description = Natywna implementacja KDE Connect dla pulpitu COSMIC
applet-settings = Ustawienia

# Pairing
pairing-requests = Parowanie
pairing-wants-to-pair = Prosi o parowanie
pairing-accept = Akceptuj
pairing-reject = Odrzuć

# Device list (popup)
devices-header = Urządzenia
devices-none-paired = Brak sparowanych urządzeń
devices-offline = Brak połączenia
devices-not-reachable = Urządzenie nieosiągalne

# Media section (popup)
media-header = Aktualnie odtwarzane

# Quick actions (popup)
quick-actions-header = Szybkie akcje
quick-actions-ping = Ping
quick-actions-find-phone = Znajdź mój telefon
quick-actions-share-clipboard = Udostępnij schowek
quick-actions-sms = Wiadomości SMS
quick-actions-files-header = Pliki
quick-actions-send-file = Wyślij plik
quick-actions-browse-device = Przeglądaj urządzenie
quick-actions-unmount-device = Odmontuj urządzenie
quick-actions-run-commands-header = Uruchom komendy

# Run Command management (settings)
run-commands-manage-header = Komendy (uruchomione z telefonu)
run-commands-add-header = Dodaj nową komende
run-commands-name-placeholder = Nazwa (np. Wygaszacz ekranu)
run-commands-command-placeholder = Komendy powłoki (np. loginctl lock-session)
run-commands-add-button = Dodaj komende
run-commands-delete = Usuń

# Settings window
settings-title = KDE Connect Ustawienia
settings-tab-paired = Sparowane urządzenia
settings-tab-available = Dostępne urządzenia
settings-scan-again = Skanuj ponownie

# Paired devices tab
paired-devices-header = Sparowane urządzenia
paired-devices-none =
    Brak sparowanych urządzeń.
    Użyj zakładki Dostępne urządzenia aby sparować
paired-devices-connected = Połączono
paired-devices-offline = Brak połączenia
paired-devices-unpair = Odparuj
paired-plugins-header = Ustawienia pluginów
paired-plugins-hint = Wybierz sparowane urządzenie aby skonfigurować pluginy.

# Available devices tab
available-devices-header = Dostępne urządzenia
available-devices-hint = Urządzenia w sieci lokalnej, które nie zostały sparowane.
available-devices-none = Nie znaleziono urządzeń
available-devices-none-hint = Upewnij się, że urządzenie jest w tej samej sieci co KDE Connect.
available-devices-pair = Paruj
available-devices-pairing = Parowanie…

# Plugin names and descriptions
plugin-battery-name = Monitor baterii
plugin-battery-desc = Wyświetl poziom baterii telefonu i status ładowania na panelu.
plugin-clipboard-name = Synchronizacja schowka
plugin-clipboard-desc = Automatycznie synchronizuj schowek między pulpitem a telefonem.
plugin-connectivity-name = Raporty dotyczące łączności
plugin-connectivity-desc = Pokaż siłe sygnału i typ sieci urządzenia mobilnego (4G, 5G, itd.).
plugin-contacts-name = Kontakty
plugin-contacts-desc = Synchronizuj konatakty, aby wiadomości SMS pokazywały nazwy kontaktu zamiast numeru.
plugin-findmyphone-name = Znajdź moje urządzenie
plugin-findmyphone-desc = Zadzwoń telefonem z pełną głośnością, aby go zlokalizować.
plugin-mpris-name = Kontrola mediów
plugin-mpris-desc = Kontroluj odtwarzanie mediów na twoim telefonie prosto z pulpitu.
plugin-notifications-name = Powiadomienia
plugin-notifications-desc = Otrzymuj powiadomienia z telefonu na pulpicie.
plugin-ping-name = Ping
plugin-ping-desc = Wysyłaj i otrzymuj powiadomienia ping, w celu weryfikacji poprawności połączenia.
plugin-runcommand-name = Uruchamianie komend
plugin-runcommand-desc = Uruchamiaj komendy na pulpicie prosto z telefonu.
plugin-share-name = Udostępniaj pliki
plugin-share-desc = Wysyłaj i odbieraj pliki i adresy URL pomiędzy urządzeniami.
plugin-sms-name = Wiadomości SMS
plugin-sms-desc = Wysyłaj i odbieraj wiadomości SMS z pulpitu komputera.
plugin-systemvolume-name = Głośność systemu
plugin-systemvolume-desc = Kontroluj głośność systemu oraz wyciszaj z telefonu.
plugin-telephony-name = Połączenia
plugin-telephony-desc = Pokazuj powiadomienia na pulpicie o nadchodzących, nieodebranych, bądź aktywnych połączeniach z telefonu.

# SMS app
sms-window-title = SMS - { $device }
sms-search-placeholder = Szukaj dyskusji…
sms-open-attachment = Otwórz
sms-attach-picker-title = Załącz plik
sms-attachment-photo = Zdjęcie
sms-attachment-video = Wideo
sms-attachment-generic = Załącznik
sms-save-attachment-title = Zapisz załącznik
sms-save-success-summary = Załącznik zapisany
sms-save-failed-summary = Zapisanie załącznika niepowiodło się
sms-no-conversations = Brak dyskusji
sms-no-matching-conversations = Brak powiązanych dyskusji
sms-select-conversation = Wybierz konwersacje, aby zobaczyć wiadomości
sms-conversation-not-found = Nie znaleziono konwersacji
sms-waiting-for-messages = Oczekiwanie na wiadomości…
sms-messages-will-appear = Wiadomości zostaną wyświetlone po synchronizacji
sms-message-placeholder = Napisz wiadomość…
sms-send = Wyślij

# SMS new chat dialog
sms-new-chat-title = Rozpocznij czat
sms-new-chat-prompt = Wpisz numer telefonu bądź nazwe kontaktu:
sms-new-chat-contacts = Kontakty
sms-new-chat-cancel = Anuluj
sms-new-chat-start = Zacznij 
sms-new-chat-no-contacts = Brak kontaktów
sms-new-chat-no-matches = Brak powiązanych kontaktów
sms-new-chat-showing = Pokaż { $count } kontakt{ $count ->
    [one] {""}
    *[other] y
}

# SMS delete confirmation dialog
sms-delete-confirm-title = Usunąc konwersacje?
sms-delete-confirm-body = Czy chcesz usunąć konwersacje z { $name }, tylko z tego urządzenia? — to nie usunie jej z telefonu, a nowe wiadomości zostaną ukryte.
sms-delete-confirm-action = Usuń
sms-delete-confirm-cancel = Anuluj

# Emoji categories
emoji-smileys = Uśmieszki
emoji-people = Ludzie
emoji-animals = Zwierzęta
emoji-food = Jedzenie
emoji-travel = Podróże
emoji-activities = Aktywności
emoji-objects = Obiekty
emoji-symbols = Symbole

# Relative timestamps
time-just-now = Teraz
time-week-ago = Ponad tydzień temu

# Misc
device-unknown = Nieznane urządzenie
file-picker-title = Wybierz pliki do wysłania

# Notifications
notification-pairing-summary = Parowanie
