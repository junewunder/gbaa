# The factory's recipes (status, next, loop, fleet, long, …) come from the wfactory
# skill: its runtime/factory.just, through the link .tracker/skill that factory/skill keeps.
# In a fresh clone, run `factory/tracker status` once to make the link.
# A recipe the project defines too is the project's: a recipe added to the skill later never
# breaks a factory's justfile. (It also means two definitions in the project's own justfile
# don't fail: the last one wins.)
set allow-duplicate-recipes
import? '../.tracker/skill/runtime/factory.just'
