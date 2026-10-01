default miho_route_done = False
default noah_route_done = False
default yuno_route_done = False
default player_quest = ""

init python:
    def update_final_quest():
        names = []
        if not miho_route_done:
            names.append("Miho")
        if not noah_route_done:
            names.append("Noah")
        if not yuno_route_done:
            names.append("Yuno")
        if not names:
            store.player_quest = "All character stories completed."
            return
        store.player_quest = (
            "Visit " + ", ".join(names) + " in the morning." +
            "\n\nOnly one character story can be progressed at a time."
        )

    def unrelated_names():
        names = []
        names.append("InternalFlag")

screen quest_log():
    text player_quest

label start:
    "A normal dialogue line."
    "Miho"
    return
