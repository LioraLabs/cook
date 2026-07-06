#include <SDL3/SDL.h>

int main(int argc, char *argv[]) {
    (void)argc; (void)argv;
    if (!SDL_Init(SDL_INIT_VIDEO)) return 1;

    SDL_Window *window = SDL_CreateWindow("cook + SDL3", 800, 600, 0);
    if (!window) { SDL_Quit(); return 1; }

    SDL_Renderer *renderer = SDL_CreateRenderer(window, NULL);
    if (!renderer) { SDL_DestroyWindow(window); SDL_Quit(); return 1; }

    SDL_Event ev;
    int running = 1;
    while (running) {
        while (SDL_PollEvent(&ev)) {
            if (ev.type == SDL_EVENT_QUIT) running = 0;
            if (ev.type == SDL_EVENT_KEY_DOWN
                && ev.key.scancode == SDL_SCANCODE_ESCAPE) running = 0;
        }
        SDL_SetRenderDrawColor(renderer, 30, 30, 40, 255);
        SDL_RenderClear(renderer);
        SDL_RenderPresent(renderer);
    }

    SDL_DestroyRenderer(renderer);
    SDL_DestroyWindow(window);
    SDL_Quit();
    return 0;
}
