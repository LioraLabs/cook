/*
 * Adapted from raylib's examples/core/core_basic_window.c
 *      https://github.com/raysan5/raylib (BSD-equivalent zlib license)
 *
 * Copyright (c) 2013-2024 Ramon Santamaria (@raysan5)
 */
#include "raylib.h"

int main(void)
{
    InitWindow(800, 450, "raylib via Cook");

    SetTargetFPS(60);

    while (!WindowShouldClose())
    {
        BeginDrawing();
        ClearBackground(RAYWHITE);
        DrawText("Congrats! You made your first raylib window via Cook!", 80, 200, 20, LIGHTGRAY);
        EndDrawing();
    }

    CloseWindow();
    return 0;
}
