# A tiny Ren'Py script fixture for the translator tests.

define e = Character("Eileen")
define m = Character("Me")

label start:
    "It was a dark and stormy night."
    e "Hello. I'm glad you could make it."
    e happy "This is going to be fun!" with vpunch
    voice "audio/hello.ogg"
    m "Where should we begin?"

    menu:
        "Where to?"

        "The forest":
            e "Into the woods we go."
            jump forest

        "The village" if trust > 2:
            e "Back to town."
            jump village

    "She said \"watch out\" and pointed."
    return

screen ui_test():
    text "HUD text, not dialogue."
    textbutton "Menu button"

init python:
    greeting = "python code string"
