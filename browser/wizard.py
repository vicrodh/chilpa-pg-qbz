"""Native setup window with executable pickers and an explicit Firefox launch."""

import json
import os
from pathlib import Path
import shutil
import shlex
import sys
from PySide6.QtCore import QProcess, QTimer, QUrl
from PySide6.QtGui import QIcon, QAction
from PySide6.QtNetwork import QNetworkAccessManager, QNetworkRequest
from PySide6.QtWidgets import (
    QApplication,
    QComboBox,
    QFileDialog,
    QHBoxLayout,
    QLabel,
    QLineEdit,
    QMainWindow,
    QMenu,
    QPushButton,
    QSystemTrayIcon,
    QVBoxLayout,
    QWidget,
)
from launch import candidates, flatpak_available, normalize


class Wizard(QMainWindow):
    def __init__(self, args):
        super().__init__()
        self.args = args
        self.setWindowTitle("QBZ Browser")
        self.resize(640, 640)
        self.child = QProcess(self)
        self.network = QNetworkAccessManager(self)
        self.code = ""
        self.connected = False
        self.preferences = args.data / "launcher.json"
        body = QWidget()
        layout = QVBoxLayout(body)
        layout.setSpacing(14)
        self.setCentralWidget(body)
        title = QLabel("QBZ")
        title.setStyleSheet("font-size:32px;font-weight:600")
        layout.addWidget(title)
        intro = QLabel(
            "Conecta Firefox con el plugin QBZ. Tendrá su propio perfil; tus otras ventanas y sesiones no se compartirán."
        )
        intro.setWordWrap(True)
        layout.addWidget(intro)
        self.kind = QComboBox()
        self.kind.addItem("Firefox instalado", "native")
        if sys.platform == "linux":
            self.kind.addItem("Firefox Flatpak", "flatpak")
        layout.addWidget(self.kind)
        self.firefox, self.firefox_button = self.picker(
            layout,
            "Firefox",
            candidates()[0] if candidates() else "",
            self.choose_firefox,
        )
        self.ffmpeg, self.ffmpeg_button = self.picker(
            layout, "FFmpeg", args.ffmpeg or "", self.choose_ffmpeg
        )
        self.permissions = QLabel()
        self.permissions.setWordWrap(True)
        layout.addWidget(self.permissions)
        self.permission_command = QLineEdit()
        self.permission_command.setReadOnly(True)
        layout.addWidget(self.permission_command)
        self.copy_permission = QPushButton("Copiar comando con este permiso")
        self.copy_permission.clicked.connect(
            lambda: QApplication.clipboard().setText(self.permission_command.text())
        )
        layout.addWidget(self.copy_permission)
        self.copy_command = QPushButton("Copiar comando de instalación")
        self.copy_command.clicked.connect(
            lambda: QApplication.clipboard().setText(
                "flatpak install --user flathub org.mozilla.firefox"
            )
        )
        layout.addWidget(self.copy_command)
        self.start = QPushButton("Abrir Firefox para iniciar sesión")
        self.start.clicked.connect(self.launch)
        layout.addWidget(self.start)
        self.open_account = QPushButton("Abrir Qobuz · cuenta y cerrar sesión")
        self.open_account.setEnabled(False)
        self.open_account.clicked.connect(self.open_browser)
        layout.addWidget(self.open_account)
        self.continue_button = QPushButton("Ya inicié sesión · Continuar")
        self.continue_button.clicked.connect(self.check)
        self.continue_button.setEnabled(False)
        layout.addWidget(self.continue_button)
        self.status = QLabel("Elige Firefox para comenzar.")
        self.status.setWordWrap(True)
        layout.addWidget(self.status)
        self.copy = QPushButton("Copiar código para el plugin QBZ")
        self.copy.setEnabled(False)
        self.copy.clicked.connect(self.copy_code)
        layout.addWidget(self.copy)
        note = QLabel(
            "En Chilpa, abre los ajustes de QBZ, pega el código y pulsa Continuar. El auxiliar debe seguir abierto mientras uses QBZ."
        )
        note.setWordWrap(True)
        layout.addWidget(note)
        self.kind.currentIndexChanged.connect(self.update_mode)
        self.timer = QTimer(self)
        self.timer.setInterval(1000)
        self.timer.timeout.connect(self.await_connection)
        self.child.finished.connect(self.exited)
        self.child.errorOccurred.connect(lambda _: self.exited())
        icon = QIcon(str(Path(__file__).resolve().parent / "icon.png"))
        self.setWindowIcon(icon)
        self.tray = None
        if QSystemTrayIcon.isSystemTrayAvailable():
            self.tray = QSystemTrayIcon(icon, self)
            menu = QMenu()
            open_action = menu.addAction("Abrir QBZ Browser")
            open_action.triggered.connect(self.showNormal)
            quit_action = menu.addAction("Cerrar QBZ Browser")
            quit_action.triggered.connect(self.quit)
            self.tray.setContextMenu(menu)
            self.tray.show()
        if self.preferences.exists():
            try:
                saved = json.loads(self.preferences.read_text())
                self.firefox.setText(saved.get("firefox", self.firefox.text()))
                self.ffmpeg.setText(saved.get("ffmpeg", self.ffmpeg.text()))
                self.kind.setCurrentIndex(
                    max(0, self.kind.findData(saved.get("mode", "native")))
                )
            except (ValueError, OSError):
                pass
        self.update_mode()

    def picker(self, layout, label, value, callback):
        layout.addWidget(QLabel(label))
        row = QHBoxLayout()
        field = QLineEdit(value)
        field.setReadOnly(True)
        field.setCursorPosition(0)
        field.setToolTip(value)
        field.setPlaceholderText("Selecciona la aplicación…")
        row.addWidget(field)
        button = QPushButton("Elegir…")
        button.clicked.connect(callback)
        row.addWidget(button)
        layout.addLayout(row)
        return field, button

    def choose_firefox(self):
        if sys.platform == "darwin":
            path = QFileDialog.getExistingDirectory(
                self,
                "Seleccionar Firefox.app",
                "/Applications",
                QFileDialog.ShowDirsOnly | QFileDialog.DontResolveSymlinks,
            )
        else:
            path, _ = QFileDialog.getOpenFileName(
                self,
                "Seleccionar Firefox",
                str(Path.home()),
                "Firefox (firefox firefox.exe);;Aplicaciones (*)",
            )
        if path:
            self.firefox.setText(str(normalize(path)))

    def choose_ffmpeg(self):
        path, _ = QFileDialog.getOpenFileName(
            self, "Seleccionar FFmpeg", str(Path.home()), "Ejecutables (*)"
        )
        if path:
            self.ffmpeg.setText(path)

    def update_mode(self):
        flatpak = self.kind.currentData() == "flatpak"
        self.firefox.setEnabled(not flatpak)
        self.firefox_button.setEnabled(not flatpak)
        installed = flatpak_available() if flatpak else False
        self.copy_command.setVisible(flatpak and not installed)
        self.permission_command.setVisible(flatpak)
        self.copy_permission.setVisible(flatpak)
        self.permission_command.setText(
            shlex.join(
                [
                    "flatpak",
                    "run",
                    "--filesystem=" + str(self.args.profile.resolve()),
                    "org.mozilla.firefox",
                    "--no-remote",
                    "--profile",
                    str(self.args.profile.resolve()),
                ]
            )
        )
        self.permission_command.setCursorPosition(0)
        self.permissions.setText(
            (
                "Firefox Flatpak tendrá acceso únicamente al perfil de QBZ mediante un permiso para esta ejecución. No se añaden permisos permanentes."
                if installed
                else "Firefox Flatpak no está instalado. Copia y ejecuta el comando, y vuelve a abrir este asistente."
            )
            if flatpak
            else "QBZ usa un perfil propio y una conexión autenticada en este equipo. Chilpa Flatpak no necesita acceso a los archivos de Firefox."
        )

    def launch(self):
        if not Path(self.ffmpeg.text()).is_file():
            self.status.setText("Selecciona FFmpeg antes de continuar.")
            return
        if (
            self.kind.currentData() == "native"
            and not Path(self.firefox.text()).is_file()
        ):
            self.status.setText("Selecciona Firefox antes de continuar.")
            return
        if self.child.state() != QProcess.NotRunning:
            self.check()
            return
        self.args.data.mkdir(parents=True, exist_ok=True)
        self.args.data.chmod(0o700)
        self.preferences.write_text(
            json.dumps(
                {
                    "mode": self.kind.currentData(),
                    "firefox": self.firefox.text(),
                    "ffmpeg": self.ffmpeg.text(),
                }
            )
        )
        script = (
            []
            if getattr(sys, "frozen", False)
            else [str(Path(__file__).resolve().parent / "companion.py")]
        )
        arguments = script + [
            "--worker",
            "--data",
            str(self.args.data),
            "--cache",
            str(self.args.cache),
            "--profile",
            str(self.args.profile),
            "--mode",
            self.kind.currentData(),
            "--firefox",
            self.firefox.text(),
            "--ffmpeg",
            self.ffmpeg.text(),
            "--port",
            str(self.args.port),
        ]
        self.child.setProgram(sys.executable)
        self.child.setArguments(arguments)
        self.child.start()
        self.start.setEnabled(False)
        self.status.setText("Abriendo el perfil de QBZ en Firefox…")
        self.timer.start()

    def await_connection(self):
        if (self.args.data / "connection.json").exists():
            self.continue_button.setEnabled(True)
            self.open_account.setEnabled(True)
            self.timer.stop()
            self.status.setText(
                "Inicia sesión en la pestaña de Qobuz y pulsa Continuar aquí. Si ya tenías sesión, solo pulsa Continuar."
            )

    def open_browser(self):
        try:
            config = json.loads((self.args.data / "connection.json").read_text())
        except (OSError, ValueError):
            self.status.setText("Primero abre QBZ Browser.")
            return
        request = QNetworkRequest(QUrl(f"http://127.0.0.1:{config['port']}/open"))
        request.setRawHeader(b"Authorization", ("Bearer " + config["key"]).encode())
        request.setHeader(QNetworkRequest.ContentTypeHeader, "application/json")
        request.setTransferTimeout(30000)
        reply = self.network.post(request, b"{}")
        self.open_account.setEnabled(False)

        def finished():
            okay = reply.attribute(QNetworkRequest.HttpStatusCodeAttribute) == 200
            reply.readAll()
            reply.deleteLater()
            self.open_account.setEnabled(True)
            self.status.setText(
                "Usa el menú de cuenta de Qobuz. Si cambias de cuenta, desconecta y vuelve a conectar el plugin."
                if okay
                else "Espera a que termine la preparación de audio y vuelve a intentarlo."
            )

        reply.finished.connect(finished)

    def check(self):
        try:
            config = json.loads((self.args.data / "connection.json").read_text())
        except (OSError, ValueError):
            self.status.setText("Abre QBZ Browser antes de continuar.")
            return
        request = QNetworkRequest(QUrl(f"http://127.0.0.1:{config['port']}/session"))
        request.setRawHeader(b"Authorization", ("Bearer " + config["key"]).encode())
        request.setHeader(QNetworkRequest.ContentTypeHeader, "application/json")
        request.setTransferTimeout(15000)
        reply = self.network.post(request, b"{}")
        self.continue_button.setEnabled(False)

        def finished():
            okay = reply.attribute(QNetworkRequest.HttpStatusCodeAttribute) == 200
            reply.readAll()
            reply.deleteLater()
            self.continue_button.setEnabled(True)
            if okay:
                self.code = f"qbz1:{config['port']}:{config['key']}"
                self.copy.setEnabled(True)
                self.connected = True
                self.status.setText(
                    "Firefox conectado. Copia el código y úsalo en los ajustes del plugin QBZ."
                )
            else:
                self.status.setText(
                    "La sesión todavía no está lista. Termina de iniciar sesión en Firefox y vuelve a pulsar Continuar."
                )

        reply.finished.connect(finished)

    def copy_code(self):
        QApplication.clipboard().setText(self.code)
        self.status.setText("Código copiado. Pégalo en los ajustes del plugin QBZ.")

    def exited(self, *_):
        self.timer.stop()
        self.open_account.setEnabled(False)
        self.start.setEnabled(True)
        self.copy.setEnabled(False)
        self.continue_button.setEnabled(False)
        self.status.setText("QBZ Browser se cerró. Puedes volver a abrirlo aquí.")
        self.code = ""

    def closeEvent(self, event):
        if self.tray and self.child.state() != QProcess.NotRunning:
            self.hide()
            event.ignore()
        else:
            self.quit()
            event.accept()

    def quit(self):
        self.child.terminate()
        if not self.child.waitForFinished(10000):
            self.child.kill()
            self.child.waitForFinished(2000)
        QApplication.quit()


def show(args):
    app = QApplication.instance() or QApplication(sys.argv)
    app.setApplicationName("QBZ Browser")
    wizard = Wizard(args)
    wizard.show()
    return app.exec()
