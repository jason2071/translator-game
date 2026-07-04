; TyranoScript sample scenario for round-trip tests.
[cm]
[chara_new name="akane" storage="akane.png" jname="Akane"]
[chara_new name="yamato" storage="yamato.png" jname="Yamato"]

*start
[bg storage="room.jpg" time=1000]
It was a dark and stormy night.[l][r]

#akane
Hello. I'm glad you could make it.[l][cm]
Welcome, [emb exp="f.name"]. This is going to be fun![p]

#
The room fell silent for a moment.[l]

*menu
Where should we begin?[l]
[glink text="The forest" target="*forest" size=24]
[glink text="The village" target="*village" size=24]
[s]

*forest
#yamato
Into the woods we go.[l]
[link target="*start"]Back to town.[endlink]
@jump storage=first.ks target=*village

*village
#yamato
Back to the village square.[l]

[iscript]
f.secret = "python-ish code string, not dialogue";
tf.count = tf.count + 1;
[endscript]

The end.[l]
[jump storage=next.ks target=*start]
